use std::path::PathBuf;
use std::sync::Weak;

use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    pipeline::PipelineDef,
    stage::{Stage, StageContext, StageType},
};
use qorchestrate_executor::PipelineExecutor;

pub struct PipelineCallStage {
    /// `Weak` breaks the `Registry → Stage → Executor → Registry` cycle.
    /// The executor is constructed via `Arc::new_cyclic` so this weak ref
    /// is valid for the executor's full lifetime.
    executor: Weak<PipelineExecutor>,
    templates_dir: PathBuf,
}

impl PipelineCallStage {
    pub fn new(executor: Weak<PipelineExecutor>, templates_dir: PathBuf) -> Self {
        Self {
            executor,
            templates_dir,
        }
    }
}

#[async_trait]
impl Stage for PipelineCallStage {
    fn stage_type(&self) -> StageType {
        StageType::PipelineCall
    }

    fn timeout_secs(&self) -> u64 {
        600
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        if ctx.nesting_depth >= 5 {
            return Err(StageError::InvalidInput(format!(
                "Max pipeline nesting depth (5) exceeded at depth {}",
                ctx.nesting_depth
            )));
        }

        let template_name = input
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                StageError::InvalidInput(
                    "pipeline_call requires 'template' param".to_string(),
                )
            })?;

        let toml_path = self.templates_dir.join(format!("{}.toml", template_name));
        let toml_str = std::fs::read_to_string(&toml_path).map_err(|e| {
            StageError::InvalidInput(format!("template '{}' not found: {}", template_name, e))
        })?;

        let def = PipelineDef::from_toml(&toml_str).map_err(|e| {
            StageError::InvalidInput(format!("template parse error: {}", e))
        })?;

        let _child_ctx = ctx.nested(uuid::Uuid::now_v7());
        let params = input.clone();
        let brain_path = ctx.brain_path.to_string_lossy().to_string();

        let executor = self.executor.upgrade().ok_or_else(|| {
            StageError::BackendError("PipelineExecutor dropped before stage execution".into())
        })?;
        let run_state = executor
            .run_pipeline(&def, params, brain_path)
            .await
            .map_err(|e| StageError::BackendError(e.to_string()))?;

        let output = run_state.output.unwrap_or_else(|| {
            json!({
                "pipeline_run_id": run_state.id.to_string(),
                "status": "completed"
            })
        });

        Ok(output)
    }
}
