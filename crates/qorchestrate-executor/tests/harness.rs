//! Integration harness — exercises the full pipeline executor infrastructure
//! with custom mock registries. No live HTTP calls and no external processes.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{json, Value};
use uuid::Uuid;

use qorchestrate_core::{
    errors::StageError,
    pipeline::PipelineDef,
    stage::{Stage, StageContext, StageType},
};
use qorchestrate_executor::{CheckpointStore, PipelineExecutor, PipelineStatus, StageRegistry};

// ─────────────────────────────────────────────────────────────────────────────
// Helper
// ─────────────────────────────────────────────────────────────────────────────

fn unique_checkpoint() -> Arc<CheckpointStore> {
    let dir = format!(
        "/tmp/qorchestrate_harness_{}",
        Uuid::now_v7().simple()
    );
    Arc::new(CheckpointStore::new(&dir).expect("checkpoint store"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// An empty pipeline (no stages) must complete immediately with status Completed
/// and an empty stages map.
#[tokio::test]
async fn test_empty_pipeline_completes() {
    const TOML: &str = r#"
[pipeline]
name = "empty"
version = "1.0"
"#;
    let def = PipelineDef::from_toml(TOML).expect("parse TOML");
    let checkpoint = unique_checkpoint();
    let registry = StageRegistry::new();
    let executor = Arc::new(PipelineExecutor::new(
        Arc::new(registry),
        checkpoint,
        "http://localhost:8765",
        "http://localhost:8420",
    ));

    let state = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await
        .expect("empty pipeline must complete without error");

    assert_eq!(
        state.status,
        PipelineStatus::Completed,
        "empty pipeline must reach Completed"
    );
    assert!(
        state.stages.is_empty(),
        "empty pipeline must have no stage states"
    );
}

/// Pipeline params supplied to `run_pipeline` must be forwarded verbatim into
/// the input `Value` that each stage receives.
#[tokio::test]
async fn test_pipeline_params_available_in_output() {
    const TOML: &str = r#"
[pipeline]
name = "params-test"
version = "1.0"

[[stage]]
id = "only"
type = "qpudidp_rmflow"
depends_on = []
"#;

    /// Stage that records the exact `input` it received.
    struct CapturingStage {
        captured: Arc<Mutex<Option<Value>>>,
    }

    #[async_trait]
    impl Stage for CapturingStage {
        fn stage_type(&self) -> StageType {
            StageType::QpudidpRmflow
        }

        fn timeout_secs(&self) -> u64 {
            5
        }

        async fn execute_raw(
            &self,
            input: Value,
            _ctx: &StageContext,
        ) -> Result<Value, StageError> {
            *self.captured.lock().expect("lock") = Some(input.clone());
            Ok(json!({"captured": true}))
        }
    }

    let captured = Arc::new(Mutex::new(None::<Value>));
    let def = PipelineDef::from_toml(TOML).expect("parse TOML");
    let checkpoint = unique_checkpoint();
    let mut registry = StageRegistry::new();
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(CapturingStage { captured: captured.clone() }),
    );

    let executor = Arc::new(PipelineExecutor::new(
        Arc::new(registry),
        checkpoint,
        "http://localhost:8765",
        "http://localhost:8420",
    ));

    let params = json!({
        "qubit_frequency_ghz": 5.0,
        "topology": "heavy_hex"
    });
    executor
        .run_pipeline(&def, params, "/tmp/test.brain")
        .await
        .expect("pipeline must succeed");

    let input = captured
        .lock()
        .expect("lock")
        .clone()
        .expect("stage must have been called");

    assert_eq!(
        input.get("qubit_frequency_ghz").and_then(Value::as_f64),
        Some(5.0),
        "qubit_frequency_ghz must propagate to stage input"
    );
    assert_eq!(
        input.get("topology").and_then(Value::as_str),
        Some("heavy_hex"),
        "topology must propagate to stage input"
    );
}
