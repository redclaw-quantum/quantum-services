use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Semaphore;

use qorchestrate_core::{
    errors::StageError,
    pipeline::PipelineDef,
    stage::{Stage, StageContext, StageType},
};
use qorchestrate_executor::PipelineExecutor;

pub struct BatchStage {
    /// `Weak` breaks the `Registry → Stage → Executor → Registry` cycle.
    /// The executor is constructed via `Arc::new_cyclic` so this weak ref
    /// is valid for the executor's full lifetime.
    executor: Weak<PipelineExecutor>,
    templates_dir: PathBuf,
}

impl BatchStage {
    pub fn new(executor: Weak<PipelineExecutor>, templates_dir: PathBuf) -> Self {
        Self {
            executor,
            templates_dir,
        }
    }
}

#[async_trait]
impl Stage for BatchStage {
    fn stage_type(&self) -> StageType {
        StageType::Batch
    }

    fn timeout_secs(&self) -> u64 {
        1800
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let template_name = input
            .get("inner_template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                StageError::InvalidInput(
                    "batch requires 'inner_template' param".to_string(),
                )
            })?
            .to_string();

        let batch_over_key = input
            .get("batch_over")
            .and_then(|v| v.as_str())
            .unwrap_or("items");

        let items = input
            .get(batch_over_key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let max_concurrency = input
            .get("max_concurrency")
            .and_then(|v| v.as_u64())
            .unwrap_or(4) as usize;

        let toml_path = self.templates_dir.join(format!("{}.toml", template_name));
        let toml_str = std::fs::read_to_string(&toml_path).map_err(|e| {
            StageError::InvalidInput(format!("template '{}' not found: {}", template_name, e))
        })?;

        let def: Arc<PipelineDef> = Arc::new(PipelineDef::from_toml(&toml_str).map_err(|e| {
            StageError::InvalidInput(format!("template parse error: {}", e))
        })?);

        let semaphore = Arc::new(Semaphore::new(max_concurrency));
        // Each spawned task returns (item_index, item, run_state) so we can
        // pair each result with the input that produced it. FuturesUnordered
        // completion order is non-deterministic; we sort by item_index at the
        // end to give callers a stable, input-aligned `results` list.
        let mut futures: FuturesUnordered<
            tokio::task::JoinHandle<(
                usize,
                Value,
                Result<qorchestrate_executor::PipelineRunState, qorchestrate_core::errors::PipelineError>,
            )>,
        > = FuturesUnordered::new();

        for (idx, item) in items.into_iter().enumerate() {
            let executor = self.executor.upgrade().ok_or_else(|| {
                StageError::BackendError(
                    "PipelineExecutor dropped before batch dispatch".into(),
                )
            })?;
            let def = def.clone();
            let brain_path = ctx.brain_path.to_string_lossy().to_string();
            let sem = semaphore.clone();
            let mut params = input.clone();
            if let Value::Object(ref mut m) = params {
                if let Value::Object(item_map) = &item {
                    m.extend(item_map.clone());
                } else {
                    m.insert("item".to_string(), item.clone());
                }
            }
            let item_for_result = item.clone();

            futures.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let run_result = executor.run_pipeline(&def, params, brain_path).await;
                (idx, item_for_result, run_result)
            }));
        }

        let mut indexed: Vec<(usize, Value)> = Vec::new();
        while let Some(join_result) = futures.next().await {
            match join_result {
                Ok((idx, item, Ok(state))) => indexed.push((
                    idx,
                    json!({
                        "item": item,
                        "output": state.output.unwrap_or(json!({"status": "completed"})),
                    }),
                )),
                Ok((idx, item, Err(e))) => indexed.push((
                    idx,
                    json!({ "item": item, "error": e.to_string() }),
                )),
                Err(e) => indexed.push((
                    usize::MAX,
                    json!({ "error": format!("join error: {e}") }),
                )),
            }
        }

        // Sort so `results[i]` corresponds to `items[i]` regardless of
        // completion order.
        indexed.sort_by_key(|(idx, _)| *idx);
        let results: Vec<Value> = indexed.into_iter().map(|(_, v)| v).collect();

        let count = results.len();
        Ok(json!({ "results": results, "count": count }))
    }
}
