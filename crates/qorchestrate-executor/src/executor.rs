use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use serde_json::Value;
use tokio::sync::{broadcast, Semaphore};
use uuid::Uuid;

use qorchestrate_core::{
    dag::DagBuilder,
    errors::{PipelineError, StageError},
    event::StageEvent,
    pipeline::{PipelineDef, StageSpec},
    stage::StageContext,
};

use crate::{
    cache::StageResultCache,
    checkpoint::CheckpointStore,
    registry::StageRegistry,
    runner::StageRunner,
    state::{PipelineRunState, PipelineStatus, StageRunState, StageRunStatus},
};

pub struct PipelineExecutor {
    registry: Arc<StageRegistry>,
    checkpoint: Arc<CheckpointStore>,
    quantum_api_url: String,
    qpudidp_url: String,
}

impl PipelineExecutor {
    pub fn new(
        registry: Arc<StageRegistry>,
        checkpoint: Arc<CheckpointStore>,
        quantum_api_url: impl Into<String>,
        qpudidp_url: impl Into<String>,
    ) -> Self {
        Self {
            registry,
            checkpoint,
            quantum_api_url: quantum_api_url.into(),
            qpudidp_url: qpudidp_url.into(),
        }
    }

    /// Submit and run a pipeline. Returns the final pipeline run state.
    pub async fn run_pipeline(
        &self,
        def: &PipelineDef,
        params: Value,
        brain_path: impl Into<String>,
    ) -> Result<PipelineRunState, PipelineError> {
        let run_id = Uuid::now_v7();
        let brain_path = brain_path.into();
        let mut run_state =
            PipelineRunState::new(run_id, &def.meta.name, &brain_path, params.clone());

        // Initialise all stage states.
        for stage in &def.stages {
            run_state
                .stages
                .insert(stage.id.clone(), StageRunState::new(&stage.id));
        }

        self.checkpoint
            .save(&run_state)
            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;

        let batches = DagBuilder::topological_batches(&def.stages)?;

        let (event_tx, _) = broadcast::channel::<StageEvent>(256);
        let mut cache = StageResultCache::new();
        let mut stage_outputs: HashMap<String, Value> = HashMap::new();

        run_state.status = PipelineStatus::Running;
        run_state.started_at = Some(chrono::Utc::now());
        self.checkpoint
            .save(&run_state)
            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;

        let semaphore = Arc::new(Semaphore::new(def.meta.max_concurrency));
        let runner = Arc::new(StageRunner::new(self.registry.clone()));

        for batch in &batches {
            let mut futures: FuturesUnordered<_> = FuturesUnordered::new();

            for stage_id in batch {
                let spec = def
                    .stages
                    .iter()
                    .find(|s| &s.id == stage_id)
                    .expect("DAG guarantees all stage IDs are valid");

                // Already cached (resume path).
                if let Some(cached) = cache.get(run_id, stage_id) {
                    stage_outputs.insert(stage_id.clone(), cached.clone());
                    continue;
                }

                let condition_passed = spec
                    .condition
                    .as_ref()
                    .map(|c| c.evaluate(&stage_outputs))
                    .unwrap_or(true);

                let input = self.build_stage_input(spec, &params, &stage_outputs);

                let stage_id_clone = stage_id.clone();
                let spec_clone = spec.clone();
                let ctx = StageContext::new(
                    run_id,
                    stage_id.clone(),
                    self.quantum_api_url.clone(),
                    self.qpudidp_url.clone(),
                    PathBuf::from(&brain_path),
                    event_tx.clone(),
                );
                let runner_clone = runner.clone();
                let sem = semaphore.clone();

                futures.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore not closed");
                    let mut stage_state = StageRunState::new(&stage_id_clone);
                    let result = runner_clone
                        .run(&spec_clone, input, &ctx, &mut stage_state, condition_passed)
                        .await;
                    (stage_id_clone, stage_state, result)
                }));
            }

            let mut batch_failed = false;

            while let Some(join_result) = futures.next().await {
                match join_result {
                    Ok((stage_id, stage_state, Ok(output))) => {
                        cache.put(run_id, &stage_id, output.clone());
                        stage_outputs.insert(stage_id.clone(), output);
                        run_state.stages.insert(stage_id.clone(), stage_state);
                        self.checkpoint
                            .save(&run_state)
                            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;
                    }
                    Ok((stage_id, stage_state, Err(StageError::SkippedByCondition))) => {
                        stage_outputs.insert(stage_id.clone(), Value::Null);
                        run_state.stages.insert(stage_id.clone(), stage_state);
                    }
                    Ok((stage_id, stage_state, Err(e))) => {
                        run_state.stages.insert(stage_id.clone(), stage_state);
                        self.checkpoint
                            .save(&run_state)
                            .map_err(|e2| PipelineError::CheckpointError(e2.to_string()))?;
                        batch_failed = true;
                        tracing::error!("Stage '{}' failed: {}", stage_id, e);
                    }
                    Err(join_err) => {
                        tracing::error!("Stage task panicked: {}", join_err);
                        batch_failed = true;
                    }
                }
            }

            if batch_failed {
                run_state.status = PipelineStatus::Failed;
                run_state.completed_at = Some(chrono::Utc::now());
                self.checkpoint
                    .save(&run_state)
                    .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;
                return Err(PipelineError::StageFailed {
                    stage: "batch".to_string(),
                    reason: "one or more stages failed".to_string(),
                });
            }
        }

        let output = if let Some(output_spec) = &def.meta.output {
            stage_outputs.get(&output_spec.stage).cloned()
        } else {
            None
        };

        run_state.status = PipelineStatus::Completed;
        run_state.completed_at = Some(chrono::Utc::now());
        run_state.output = output;
        self.checkpoint
            .save(&run_state)
            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;

        Ok(run_state)
    }

    /// Resume a pipeline from its last checkpoint.
    /// Completed stages are skipped; failed/pending stages are re-executed.
    pub async fn resume_pipeline(
        &self,
        run_id: Uuid,
        def: &PipelineDef,
    ) -> Result<PipelineRunState, PipelineError> {
        let saved_state = self
            .checkpoint
            .load(run_id)
            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;

        let mut cache = StageResultCache::new();
        let mut stage_outputs: HashMap<String, Value> = HashMap::new();

        for (stage_id, stage_state) in &saved_state.stages {
            if stage_state.status == StageRunStatus::Completed
                && let Some(output) = &stage_state.output {
                    cache.put(run_id, stage_id, output.clone());
                    stage_outputs.insert(stage_id.clone(), output.clone());
                }
        }

        let params = saved_state.params.clone();
        let brain_path = saved_state.brain_path.clone();

        let mut run_state =
            PipelineRunState::new(run_id, &def.meta.name, &brain_path, params.clone());
        run_state.stages = saved_state.stages;
        run_state.status = PipelineStatus::Running;
        run_state.started_at = saved_state.started_at;

        let batches = DagBuilder::topological_batches(&def.stages)?;
        let (event_tx, _) = broadcast::channel::<StageEvent>(256);
        let semaphore = Arc::new(Semaphore::new(def.meta.max_concurrency));
        let runner = Arc::new(StageRunner::new(self.registry.clone()));

        for batch in &batches {
            let mut futures: FuturesUnordered<_> = FuturesUnordered::new();

            for stage_id in batch {
                let spec = def
                    .stages
                    .iter()
                    .find(|s| &s.id == stage_id)
                    .expect("DAG guarantees all stage IDs are valid");

                // Skip stages already completed or skipped.
                if let Some(stage_state) = run_state.stages.get(stage_id)
                    && (stage_state.status == StageRunStatus::Completed
                        || stage_state.status == StageRunStatus::Skipped)
                    {
                        let out = stage_state
                            .output
                            .clone()
                            .unwrap_or(Value::Null);
                        stage_outputs.insert(stage_id.clone(), out);
                        continue;
                    }

                let condition_passed = spec
                    .condition
                    .as_ref()
                    .map(|c| c.evaluate(&stage_outputs))
                    .unwrap_or(true);

                let input = self.build_stage_input(spec, &params, &stage_outputs);

                let stage_id_clone = stage_id.clone();
                let spec_clone = spec.clone();
                let ctx = StageContext::new(
                    run_id,
                    stage_id.clone(),
                    self.quantum_api_url.clone(),
                    self.qpudidp_url.clone(),
                    PathBuf::from(&brain_path),
                    event_tx.clone(),
                );
                let runner_clone = runner.clone();
                let sem = semaphore.clone();

                futures.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore not closed");
                    let mut stage_state = StageRunState::new(&stage_id_clone);
                    let result = runner_clone
                        .run(&spec_clone, input, &ctx, &mut stage_state, condition_passed)
                        .await;
                    (stage_id_clone, stage_state, result)
                }));
            }

            while let Some(join_result) = futures.next().await {
                match join_result {
                    Ok((stage_id, stage_state, Ok(output))) => {
                        cache.put(run_id, &stage_id, output.clone());
                        stage_outputs.insert(stage_id.clone(), output);
                        run_state.stages.insert(stage_id.clone(), stage_state);
                        self.checkpoint
                            .save(&run_state)
                            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;
                    }
                    Ok((stage_id, stage_state, Err(StageError::SkippedByCondition))) => {
                        stage_outputs.insert(stage_id.clone(), Value::Null);
                        run_state.stages.insert(stage_id.clone(), stage_state);
                    }
                    Ok((stage_id, stage_state, Err(e))) => {
                        run_state.stages.insert(stage_id.clone(), stage_state);
                        run_state.status = PipelineStatus::Failed;
                        self.checkpoint
                            .save(&run_state)
                            .map_err(|e2| PipelineError::CheckpointError(e2.to_string()))?;
                        return Err(PipelineError::StageFailed {
                            stage: stage_id,
                            reason: e.to_string(),
                        });
                    }
                    Err(join_err) => {
                        return Err(PipelineError::StageFailed {
                            stage: "batch".to_string(),
                            reason: join_err.to_string(),
                        });
                    }
                }
            }
        }

        let output = if let Some(output_spec) = &def.meta.output {
            stage_outputs.get(&output_spec.stage).cloned()
        } else {
            None
        };

        run_state.status = PipelineStatus::Completed;
        run_state.completed_at = Some(chrono::Utc::now());
        run_state.output = output;
        self.checkpoint
            .save(&run_state)
            .map_err(|e| PipelineError::CheckpointError(e.to_string()))?;

        Ok(run_state)
    }

    /// Build the input Value for a stage by merging pipeline params, stage-specific
    /// params, and making prior stage outputs available as `"<dep_id>_output"` keys.
    fn build_stage_input(
        &self,
        spec: &StageSpec,
        pipeline_params: &Value,
        stage_outputs: &HashMap<String, Value>,
    ) -> Value {
        let mut map = serde_json::Map::new();

        if let Value::Object(m) = pipeline_params {
            map.extend(m.clone());
        }

        for (k, v) in &spec.params {
            map.insert(k.clone(), v.clone());
        }

        for dep_id in &spec.depends_on {
            if let Some(dep_output) = stage_outputs.get(dep_id) {
                map.insert(format!("{dep_id}_output"), dep_output.clone());
            }
        }

        Value::Object(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use serde_json::json;
    use uuid::Uuid;

    use qorchestrate_core::{
        errors::StageError,
        pipeline::{PipelineDef, PipelineMeta, StageSpec},
        stage::{Stage, StageContext, StageType},
    };

    use crate::{
        checkpoint::CheckpointStore,
        registry::StageRegistry,
        state::{PipelineStatus, StageRunStatus},
    };

    // -------------------------------------------------------------------------
    // Helpers
    // -------------------------------------------------------------------------

    struct MockStage {
        stage_type: StageType,
        output: Value,
        should_fail: bool,
    }

    #[async_trait]
    impl Stage for MockStage {
        async fn execute_raw(
            &self,
            _input: Value,
            _ctx: &StageContext,
        ) -> Result<Value, StageError> {
            if self.should_fail {
                Err(StageError::BackendError("mock failure".to_string()))
            } else {
                Ok(self.output.clone())
            }
        }

        fn stage_type(&self) -> StageType {
            self.stage_type.clone()
        }

        fn timeout_secs(&self) -> u64 {
            5
        }
    }

    fn make_mock(stage_type: StageType, output: Value, should_fail: bool) -> Arc<MockStage> {
        Arc::new(MockStage {
            stage_type,
            output,
            should_fail,
        })
    }

    fn spec(
        id: &str,
        stage_type: StageType,
        deps: &[&str],
        condition: Option<qorchestrate_core::condition::ConditionExpr>,
    ) -> StageSpec {
        StageSpec {
            id: id.to_string(),
            stage_type,
            depends_on: deps.iter().map(|s| s.to_string()).collect(),
            timeout_secs: Some(5),
            params: HashMap::new(),
            condition,
            fallback: None,
            retry: None,
        }
    }

    fn two_stage_def() -> PipelineDef {
        PipelineDef {
            meta: PipelineMeta {
                name: "test-pipeline".to_string(),
                version: "0.1.0".to_string(),
                description: String::new(),
                max_concurrency: 4,
                default_timeout_secs: 60,
                output: None,
            },
            stages: vec![
                spec("stage_a", StageType::Skip, &[], None),
                spec("stage_b", StageType::TwinMock, &["stage_a"], None),
            ],
        }
    }

    fn three_stage_def() -> PipelineDef {
        PipelineDef {
            meta: PipelineMeta {
                name: "three-pipeline".to_string(),
                version: "0.1.0".to_string(),
                description: String::new(),
                max_concurrency: 4,
                default_timeout_secs: 60,
                output: None,
            },
            stages: vec![
                spec("stage_a", StageType::Skip, &[], None),
                spec("stage_b", StageType::TwinMock, &["stage_a"], None),
                spec("stage_c", StageType::Batch, &["stage_b"], None),
            ],
        }
    }

    fn temp_checkpoint() -> Arc<CheckpointStore> {
        let dir = std::env::temp_dir().join(format!(
            "qorchestrate_exec_test_{}",
            Uuid::new_v4()
        ));
        Arc::new(CheckpointStore::new(dir).expect("checkpoint store"))
    }

    // -------------------------------------------------------------------------
    // Tests
    // -------------------------------------------------------------------------

    /// 1. Two-stage pipeline runs to completion.
    #[tokio::test]
    async fn test_pipeline_run_with_mock_stages() {
        let mut registry = StageRegistry::new();
        registry.register(
            StageType::Skip,
            make_mock(StageType::Skip, json!({"result": "ok"}), false),
        );
        registry.register(
            StageType::TwinMock,
            make_mock(StageType::TwinMock, json!({"result": "ok"}), false),
        );

        let executor = PipelineExecutor::new(
            Arc::new(registry),
            temp_checkpoint(),
            "http://localhost:8080",
            "http://localhost:9090",
        );

        let def = two_stage_def();
        let result = executor
            .run_pipeline(&def, json!({}), "/brain")
            .await
            .expect("pipeline should complete");

        assert_eq!(result.status, PipelineStatus::Completed);

        let a = result.stages.get("stage_a").expect("stage_a present");
        assert_eq!(a.status, StageRunStatus::Completed);

        let b = result.stages.get("stage_b").expect("stage_b present");
        assert_eq!(b.status, StageRunStatus::Completed);
    }

    /// 2. Pipeline with a failing stage B can be resumed after B is fixed.
    #[tokio::test]
    async fn test_pipeline_resume() {
        // First run: A succeeds, B fails.
        let mut registry = StageRegistry::new();
        registry.register(
            StageType::Skip,
            make_mock(StageType::Skip, json!({"result": "ok"}), false),
        );
        registry.register(
            StageType::TwinMock,
            make_mock(StageType::TwinMock, json!({}), true), // fails
        );
        // C won't be reached in first run, but register for completeness.
        registry.register(
            StageType::Batch,
            make_mock(StageType::Batch, json!({"result": "ok"}), false),
        );

        let checkpoint = temp_checkpoint();

        let executor = PipelineExecutor::new(
            Arc::new(registry),
            checkpoint.clone(),
            "http://localhost:8080",
            "http://localhost:9090",
        );

        let def = three_stage_def();
        let first_result = executor.run_pipeline(&def, json!({}), "/brain").await;

        // First run must fail because B fails.
        assert!(first_result.is_err(), "expected first run to fail");

        // Recover the run_id by reading the single JSON file from our specific
        // checkpoint directory (we hold the Arc so we know its dir).
        let ck_dir = checkpoint.dir().to_path_buf();
        let json_files: Vec<_> = std::fs::read_dir(&ck_dir)
            .expect("read checkpoint dir")
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path()
                    .extension()
                    .map(|x| x == "json")
                    .unwrap_or(false)
            })
            .collect();

        assert!(!json_files.is_empty(), "expected at least one checkpoint file");

        // The final failed checkpoint is the most recently modified file.
        let ck_file = json_files
            .iter()
            .max_by_key(|e| e.metadata().unwrap().modified().unwrap())
            .unwrap()
            .path();

        let saved: crate::state::PipelineRunState =
            serde_json::from_str(&std::fs::read_to_string(&ck_file).unwrap())
                .expect("parse checkpoint");

        let run_id = saved.id;

        // Verify A is Completed and B is Failed in the checkpoint.
        let stage_a = saved.stages.get("stage_a").expect("stage_a in checkpoint");
        assert_eq!(stage_a.status, StageRunStatus::Completed);
        let stage_b = saved.stages.get("stage_b").expect("stage_b in checkpoint");
        assert_eq!(stage_b.status, StageRunStatus::Failed);

        // Build a second executor with B fixed.
        let mut registry2 = StageRegistry::new();
        registry2.register(
            StageType::Skip,
            make_mock(StageType::Skip, json!({"result": "ok"}), false),
        );
        registry2.register(
            StageType::TwinMock,
            make_mock(StageType::TwinMock, json!({"result": "ok"}), false), // fixed
        );
        registry2.register(
            StageType::Batch,
            make_mock(StageType::Batch, json!({"result": "ok"}), false),
        );

        let executor2 = PipelineExecutor::new(
            Arc::new(registry2),
            checkpoint,
            "http://localhost:8080",
            "http://localhost:9090",
        );

        let resumed = executor2
            .resume_pipeline(run_id, &def)
            .await
            .expect("resume should complete");

        assert_eq!(resumed.status, PipelineStatus::Completed);

        let b_final = resumed.stages.get("stage_b").expect("stage_b after resume");
        assert_eq!(b_final.status, StageRunStatus::Completed);

        let c_final = resumed.stages.get("stage_c").expect("stage_c after resume");
        assert_eq!(c_final.status, StageRunStatus::Completed);
    }

    /// 3. A stage with a condition that evaluates to false is Skipped.
    #[tokio::test]
    async fn test_stage_skipped_by_condition() {
        use qorchestrate_core::condition::{Condition, ConditionExpr, ConditionOp};

        // Condition on stage_b: stage_a.output.trigger == true (but A returns false)
        let cond = ConditionExpr::Single(Condition {
            field: "stage_a.output.trigger".to_string(),
            op: ConditionOp::Eq,
            value: json!(true),
        });

        let def = PipelineDef {
            meta: PipelineMeta {
                name: "cond-pipeline".to_string(),
                version: "0.1.0".to_string(),
                description: String::new(),
                max_concurrency: 4,
                default_timeout_secs: 60,
                output: None,
            },
            stages: vec![
                spec("stage_a", StageType::Skip, &[], None),
                // stage_b depends on stage_a and has the false condition
                StageSpec {
                    id: "stage_b".to_string(),
                    stage_type: StageType::TwinMock,
                    depends_on: vec!["stage_a".to_string()],
                    timeout_secs: Some(5),
                    params: HashMap::new(),
                    condition: Some(cond),
                    fallback: None,
                    retry: None,
                },
            ],
        };

        let mut registry = StageRegistry::new();
        // A returns trigger: false — condition on B will not pass.
        registry.register(
            StageType::Skip,
            make_mock(StageType::Skip, json!({"trigger": false}), false),
        );
        registry.register(
            StageType::TwinMock,
            make_mock(StageType::TwinMock, json!({"result": "ok"}), false),
        );

        let executor = PipelineExecutor::new(
            Arc::new(registry),
            temp_checkpoint(),
            "http://localhost:8080",
            "http://localhost:9090",
        );

        let result = executor
            .run_pipeline(&def, json!({}), "/brain")
            .await
            .expect("pipeline should complete even with a skipped stage");

        assert_eq!(result.status, PipelineStatus::Completed);

        let a = result.stages.get("stage_a").expect("stage_a");
        assert_eq!(a.status, StageRunStatus::Completed);

        let b = result.stages.get("stage_b").expect("stage_b");
        assert_eq!(b.status, StageRunStatus::Skipped);
    }
}
