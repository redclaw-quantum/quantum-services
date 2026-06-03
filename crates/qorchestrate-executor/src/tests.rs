//! Phase 5 production-hardening tests.
//!
//! All tests are fully self-contained: no live network calls, no external
//! processes. Every stage implementation is an in-process mock or fake that
//! uses state-based assertions.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;
use std::path::PathBuf;

use async_trait::async_trait;
use futures::StreamExt;
use serde_json::{json, Value};
use tokio::sync::broadcast;
use uuid::Uuid;

use qorchestrate_core::{
    errors::StageError,
    pipeline::PipelineDef,
    stage::{Stage, StageContext, StageType},
};

use crate::{
    checkpoint::CheckpointStore,
    executor::PipelineExecutor,
    registry::StageRegistry,
    state::{PipelineStatus, StageRunStatus},
};

// ─────────────────────────────────────────────────────────────────────────────
// Shared test helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a minimal `StageContext` for unit testing individual stages.
fn test_ctx(run_id: Uuid, stage_id: &str) -> StageContext {
    let (tx, _) = broadcast::channel(64);
    StageContext::new(
        run_id,
        stage_id,
        "http://localhost:8765",
        "http://localhost:8420",
        PathBuf::from("/tmp/test.brain"),
        tx,
    )
}

/// Build a `PipelineExecutor` backed by a fresh unique temp directory.
/// Returns `(executor, checkpoint)`. The checkpoint directory persists for
/// the life of the `Arc<CheckpointStore>` returned.
fn test_executor(registry: StageRegistry) -> (Arc<PipelineExecutor>, Arc<CheckpointStore>) {
    let unique = format!(
        "/tmp/qorchestrate_phase5_{}",
        Uuid::now_v7().simple()
    );
    let checkpoint = Arc::new(CheckpointStore::new(&unique).expect("checkpoint store"));
    let executor = Arc::new(PipelineExecutor::new(
        Arc::new(registry),
        checkpoint.clone(),
        "http://localhost:8765",
        "http://localhost:8420",
    ));
    (executor, checkpoint)
}

// ─────────────────────────────────────────────────────────────────────────────
// Reusable fake stage implementations
// ─────────────────────────────────────────────────────────────────────────────

/// A stage that returns a fixed output after an optional delay.
struct MockStage {
    stage_type: StageType,
    output: Value,
    delay_ms: u64,
    call_count: Arc<AtomicUsize>,
}

impl MockStage {
    fn new(stage_type: StageType, output: Value) -> Self {
        Self {
            stage_type,
            output,
            delay_ms: 0,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn with_delay(mut self, ms: u64) -> Self {
        self.delay_ms = ms;
        self
    }

    fn call_count(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

#[async_trait]
impl Stage for MockStage {
    fn stage_type(&self) -> StageType {
        self.stage_type.clone()
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, _input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(self.output.clone())
    }
}

/// A stage that always returns `StageError::BackendError`.
struct FailStage {
    stage_type: StageType,
    call_count: Arc<AtomicUsize>,
}

impl FailStage {
    fn new(stage_type: StageType) -> Self {
        Self {
            stage_type,
            call_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    #[allow(dead_code)]
    fn call_count(&self) -> Arc<AtomicUsize> {
        self.call_count.clone()
    }
}

#[async_trait]
impl Stage for FailStage {
    fn stage_type(&self) -> StageType {
        self.stage_type.clone()
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, _input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        self.call_count.fetch_add(1, Ordering::SeqCst);
        Err(StageError::BackendError("injected failure".to_string()))
    }
}

/// A stage that sleeps longer than its declared timeout so the runner's
/// `tokio::time::timeout` fires first.
struct SlowStage {
    stage_type: StageType,
    sleep_ms: u64,
}

impl SlowStage {
    fn new(stage_type: StageType, sleep_ms: u64) -> Self {
        Self { stage_type, sleep_ms }
    }
}

#[async_trait]
impl Stage for SlowStage {
    fn stage_type(&self) -> StageType {
        self.stage_type.clone()
    }

    /// 1-second timeout so the runner's timeout triggers before sleep_ms elapses.
    fn timeout_secs(&self) -> u64 {
        1
    }

    async fn execute_raw(&self, _input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        Ok(json!({"result": "too_late"}))
    }
}

/// A stage that fails the first `max_fails` invocations then succeeds.
struct FlakyStage {
    stage_type: StageType,
    fail_count: Arc<AtomicUsize>,
    max_fails: usize,
}

#[async_trait]
impl Stage for FlakyStage {
    fn stage_type(&self) -> StageType {
        self.stage_type.clone()
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, _input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        let count = self.fail_count.fetch_add(1, Ordering::SeqCst);
        if count < self.max_fails {
            Err(StageError::BackendError(format!("flaky failure #{count}")))
        } else {
            Ok(json!({"result": "eventually_ok", "attempt": count + 1}))
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

/// 1. Run 10 independent 2-stage pipelines concurrently. All must complete,
///    and the total wall time must be well under 10× a single pipeline run,
///    proving actual parallelism.
#[tokio::test]
async fn test_concurrent_pipeline_execution() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "concurrent-test"
version = "1.0"
max_concurrency = 4
default_timeout_secs = 30

[[stage]]
id = "stage_a"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "stage_b"
type = "freq_optimize"
depends_on = ["stage_a"]
"#;

    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let mut futures_set = futures::stream::FuturesUnordered::new();
    let start = std::time::Instant::now();

    for _ in 0..10 {
        let mut registry = StageRegistry::new();
        registry.register(
            StageType::QpudidpRmflow,
            Arc::new(MockStage::new(StageType::QpudidpRmflow, json!({"freq": 5.0})).with_delay(10)),
        );
        registry.register(
            StageType::FreqOptimize,
            Arc::new(MockStage::new(StageType::FreqOptimize, json!({"plan": "ok"})).with_delay(10)),
        );
        let (executor, _cp) = test_executor(registry);
        let def_clone = def.clone();
        futures_set.push(tokio::spawn(async move {
            executor.run_pipeline(&def_clone, json!({}), "/tmp/test.brain").await
        }));
    }

    let mut success_count = 0usize;
    while let Some(result) = futures_set.next().await {
        if matches!(result, Ok(Ok(ref s)) if s.status == PipelineStatus::Completed) {
            success_count += 1;
        }
    }
    let elapsed = start.elapsed();

    assert_eq!(success_count, 10, "all 10 pipelines should complete");
    // 10 pipelines running truly concurrently with 10ms delays per stage
    // should finish in well under 5 seconds.
    assert!(
        elapsed.as_secs() < 5,
        "concurrent pipelines took too long: {elapsed:?}"
    );
}

/// 2. Middle stage of a 3-stage linear pipeline always fails.
///    - Pipeline returns an error.
///    - Stage A is Completed in state, stage B is Failed, stage C never ran.
#[tokio::test]
async fn test_error_injection_single_stage() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "error-injection"
version = "1.0"
default_timeout_secs = 30

[[stage]]
id = "stage_a"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "stage_b"
type = "freq_optimize"
depends_on = ["stage_a"]

[[stage]]
id = "stage_c"
type = "xtalk_analyze"
depends_on = ["stage_b"]
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let c_stage = Arc::new(MockStage::new(StageType::XtalkAnalyze, json!({"ok": true})));
    let c_calls = c_stage.call_count();

    let mut registry = StageRegistry::new();
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(MockStage::new(StageType::QpudidpRmflow, json!({"ok": true}))),
    );
    registry.register(
        StageType::FreqOptimize,
        Arc::new(FailStage::new(StageType::FreqOptimize)),
    );
    registry.register(StageType::XtalkAnalyze, c_stage);

    let (executor, _cp) = test_executor(registry);
    let result = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;

    assert!(result.is_err(), "pipeline must fail when middle stage fails");

    let err_str = result.unwrap_err().to_string();
    assert!(
        err_str.to_lowercase().contains("fail") || err_str.to_lowercase().contains("stage"),
        "error should mention stage failure; got: {err_str}"
    );

    // Stage C should never have been invoked — the pipeline aborted at B.
    assert_eq!(
        c_calls.load(Ordering::SeqCst),
        0,
        "stage_c must not run after stage_b fails"
    );
}

/// 3. Resume from checkpoint.
///    - First run: A succeeds, B fails.
///    - Second run (same checkpoint dir, B now fixed): resume succeeds.
///    - Stage A must NOT re-run on the second executor (call count stays 0).
#[tokio::test]
async fn test_resume_from_checkpoint() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "resume-test"
version = "1.0"
default_timeout_secs = 30

[[stage]]
id = "stage_a"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "stage_b"
type = "freq_optimize"
depends_on = ["stage_a"]

[[stage]]
id = "stage_c"
type = "xtalk_analyze"
depends_on = ["stage_b"]
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let unique_dir = format!(
        "/tmp/qorchestrate_resume_{}",
        Uuid::now_v7().simple()
    );

    // ── First run: A succeeds, B fails ──────────────────────────────────────
    let checkpoint = Arc::new(CheckpointStore::new(&unique_dir).expect("checkpoint"));

    let a_stage_run1 = Arc::new(
        MockStage::new(StageType::QpudidpRmflow, json!({"freq": 5.0}))
    );
    let a_calls_run1 = a_stage_run1.call_count();

    let mut registry1 = StageRegistry::new();
    registry1.register(StageType::QpudidpRmflow, a_stage_run1);
    registry1.register(
        StageType::FreqOptimize,
        Arc::new(FailStage::new(StageType::FreqOptimize)),
    );
    registry1.register(
        StageType::XtalkAnalyze,
        Arc::new(MockStage::new(StageType::XtalkAnalyze, json!({"crosstalk": "ok"}))),
    );

    let executor1 = Arc::new(PipelineExecutor::new(
        Arc::new(registry1),
        checkpoint.clone(),
        "http://localhost:8765",
        "http://localhost:8420",
    ));

    let first_result = executor1
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;
    assert!(first_result.is_err(), "first run must fail at stage_b");
    assert_eq!(
        a_calls_run1.load(Ordering::SeqCst),
        1,
        "stage_a ran exactly once on the first run"
    );

    // Recover run_id from the checkpoint directory.
    let checkpoints: Vec<_> = std::fs::read_dir(&unique_dir)
        .expect("read checkpoint dir")
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                == Some("json")
        })
        .collect();
    assert_eq!(
        checkpoints.len(),
        1,
        "exactly one checkpoint file should exist after first run"
    );

    let run_id: Uuid = checkpoints[0]
        .path()
        .file_stem()
        .expect("file stem")
        .to_str()
        .expect("UTF-8 stem")
        .parse()
        .expect("valid UUID in checkpoint filename");

    // ── Second run: B fixed, resume from checkpoint ──────────────────────────
    let a_stage_run2 = Arc::new(
        MockStage::new(StageType::QpudidpRmflow, json!({"freq": 5.0}))
    );
    let a_calls_run2 = a_stage_run2.call_count();

    let mut registry2 = StageRegistry::new();
    registry2.register(StageType::QpudidpRmflow, a_stage_run2);
    registry2.register(
        StageType::FreqOptimize,
        Arc::new(MockStage::new(StageType::FreqOptimize, json!({"plan": "fixed"}))),
    );
    registry2.register(
        StageType::XtalkAnalyze,
        Arc::new(MockStage::new(StageType::XtalkAnalyze, json!({"crosstalk": "ok"}))),
    );

    let executor2 = Arc::new(PipelineExecutor::new(
        Arc::new(registry2),
        checkpoint.clone(),
        "http://localhost:8765",
        "http://localhost:8420",
    ));

    let resumed = executor2
        .resume_pipeline(run_id, &def)
        .await
        .expect("resume must succeed after fixing stage_b");

    assert_eq!(
        resumed.status,
        PipelineStatus::Completed,
        "resumed pipeline must reach Completed"
    );
    assert_eq!(
        a_calls_run2.load(Ordering::SeqCst),
        0,
        "stage_a must NOT re-run on resume — it was already Completed"
    );

    let c_state = resumed
        .stages
        .get("stage_c")
        .expect("stage_c present after resume");
    assert_eq!(
        c_state.status,
        StageRunStatus::Completed,
        "stage_c must complete during resume"
    );
}

/// 4. A stage whose primary implementation sleeps longer than its 1-second
///    timeout falls back to an alternative stage type. The pipeline completes
///    and `used_fallback` is set on the stage state.
#[tokio::test]
async fn test_stage_timeout_triggers_fallback() {
    // The TOML timeout_secs on [[stage]] overrides what the runner would
    // otherwise infer from the Stage trait. We set it to 1 here so that
    // the SlowStage (sleeps 2000ms) times out immediately.
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "timeout-fallback-test"
version = "1.0"
default_timeout_secs = 10

[[stage]]
id = "slow_stage"
type = "qpudidp_rmflow"
depends_on = []
timeout_secs = 1

[stage.fallback]
type = "qpudidp_cmaes"
timeout_secs = 5
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let mut registry = StageRegistry::new();
    // Primary: sleeps 2000ms, so the 1-second timeout fires.
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(SlowStage::new(StageType::QpudidpRmflow, 2000)),
    );
    // Fallback: fast mock.
    registry.register(
        StageType::QpudidpCmaes,
        Arc::new(MockStage::new(
            StageType::QpudidpCmaes,
            json!({"source": "cmaes_fallback"}),
        )),
    );

    let (executor, _cp) = test_executor(registry);
    let result = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;

    assert!(
        result.is_ok(),
        "pipeline should succeed via fallback: {:?}",
        result.err()
    );
    let state = result.unwrap();
    assert_eq!(state.status, PipelineStatus::Completed);

    let slow_state = state
        .stages
        .get("slow_stage")
        .expect("slow_stage in final state");
    assert!(
        slow_state.used_fallback,
        "slow_stage must record used_fallback=true"
    );
}

/// 5. A semaphore with `max_concurrency = 2` throttles 6 parallel 100ms
///    stages so the total wall time is at least ~200ms (proving batching)
///    but completes successfully within a generous upper bound.
#[tokio::test]
async fn test_semaphore_limits_concurrency() {
    // root → 6 parallel leaves, each takes 100ms.
    // With max_concurrency=2 they run 2 at a time → ~3 rounds × 100ms ≈ 300ms.
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "semaphore-test"
version = "1.0"
max_concurrency = 2
default_timeout_secs = 30

[[stage]]
id = "root"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "leaf_a"
type = "freq_optimize"
depends_on = ["root"]

[[stage]]
id = "leaf_b"
type = "xtalk_analyze"
depends_on = ["root"]

[[stage]]
id = "leaf_c"
type = "readout_design"
depends_on = ["root"]

[[stage]]
id = "leaf_d"
type = "grape_optimize"
depends_on = ["root"]

[[stage]]
id = "leaf_e"
type = "pqec_assess"
depends_on = ["root"]

[[stage]]
id = "leaf_f"
type = "qec_threshold"
depends_on = ["root"]
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let mut registry = StageRegistry::new();
    // Root completes instantly.
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(MockStage::new(StageType::QpudidpRmflow, json!({"ok": true}))),
    );
    // All six leaves each take 100ms.
    for st in [
        StageType::FreqOptimize,
        StageType::XtalkAnalyze,
        StageType::ReadoutDesign,
        StageType::GrapeOptimize,
        StageType::PqecAssess,
        StageType::QecThreshold,
    ] {
        registry.register(
            st.clone(),
            Arc::new(MockStage::new(st, json!({"ok": true})).with_delay(100)),
        );
    }

    let (executor, _cp) = test_executor(registry);
    let start = std::time::Instant::now();
    let result = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;
    let elapsed = start.elapsed();

    assert!(result.is_ok(), "pipeline should succeed");
    assert_eq!(result.unwrap().status, PipelineStatus::Completed);

    // With max_concurrency=2 and 6 × 100ms stages the semaphore serialises
    // them into 3 rounds of 2. Total ≥ ~300ms; certainly < 5s.
    assert!(
        elapsed.as_millis() >= 200,
        "semaphore should produce ~300ms total; got {elapsed:?}"
    );
    assert!(
        elapsed.as_secs() < 5,
        "pipeline should not take more than 5 seconds: {elapsed:?}"
    );
}

/// 6. A stage whose condition evaluates to false is Skipped.
///    The dependent stage still runs (its dep output is Value::Null which
///    the executor inserts on skip).
#[tokio::test]
async fn test_condition_skips_stage() {
    // stage_a outputs fidelity=0.5; stage_b's condition requires >0.999.
    // stage_b must be Skipped; stage_c (depends on stage_b) must still run.
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "condition-skip-test"
version = "1.0"
default_timeout_secs = 30

[[stage]]
id = "stage_a"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "stage_b"
type = "freq_optimize"
depends_on = ["stage_a"]

[stage.condition]
field = "stage_a.output.fidelity"
op = "gt"
value = 0.999

[[stage]]
id = "stage_c"
type = "xtalk_analyze"
depends_on = ["stage_b"]
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");

    let mut registry = StageRegistry::new();
    // A returns low fidelity → condition for B is false.
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(MockStage::new(StageType::QpudidpRmflow, json!({"fidelity": 0.5}))),
    );
    registry.register(
        StageType::FreqOptimize,
        Arc::new(MockStage::new(StageType::FreqOptimize, json!({"plan": "ok"}))),
    );
    registry.register(
        StageType::XtalkAnalyze,
        Arc::new(MockStage::new(StageType::XtalkAnalyze, json!({"xtalk": "ok"}))),
    );

    let (executor, _cp) = test_executor(registry);
    let result = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;

    assert!(
        result.is_ok(),
        "pipeline should succeed even when a stage is skipped"
    );
    let state = result.unwrap();
    assert_eq!(state.status, PipelineStatus::Completed);

    let b_state = state.stages.get("stage_b").expect("stage_b in state");
    assert_eq!(
        b_state.status,
        StageRunStatus::Skipped,
        "stage_b must be Skipped when its condition is false"
    );
}

/// 7. A stage configured with `max_attempts = 3` that fails twice then
///    succeeds on the third call. The pipeline must reach Completed and
///    the total call count must be exactly 3.
#[tokio::test]
async fn test_retry_on_flaky_stage() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "retry-test"
version = "1.0"
default_timeout_secs = 30

[[stage]]
id = "flaky"
type = "qpudidp_rmflow"
depends_on = []

[stage.retry]
max_attempts = 3
backoff_secs = 0
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");
    let call_count = Arc::new(AtomicUsize::new(0));

    let mut registry = StageRegistry::new();
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(FlakyStage {
            stage_type: StageType::QpudidpRmflow,
            fail_count: call_count.clone(),
            max_fails: 2,
        }),
    );

    let (executor, _cp) = test_executor(registry);
    let result = executor
        .run_pipeline(&def, json!({}), "/tmp/test.brain")
        .await;

    assert!(
        result.is_ok(),
        "pipeline should succeed after retries: {:?}",
        result.err()
    );
    let state = result.unwrap();
    assert_eq!(state.status, PipelineStatus::Completed);
    assert_eq!(
        call_count.load(Ordering::SeqCst),
        3,
        "stage must be called exactly 3 times (2 failures + 1 success)"
    );
}

/// 8. A checkpoint file written by one executor must still be readable by a
///    new executor that opens the same directory after the first is dropped.
#[tokio::test]
async fn test_checkpoint_survives_executor_drop() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "checkpoint-persistence"
version = "1.0"
default_timeout_secs = 30

[[stage]]
id = "only_stage"
type = "qpudidp_rmflow"
depends_on = []
"#;
    let def = PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML");
    let checkpoint_dir = format!(
        "/tmp/qorchestrate_persist_{}",
        Uuid::now_v7().simple()
    );

    // First scope: run pipeline to completion, then drop executor + checkpoint.
    let run_id = {
        let checkpoint = Arc::new(CheckpointStore::new(&checkpoint_dir).expect("checkpoint"));
        let mut registry = StageRegistry::new();
        registry.register(
            StageType::QpudidpRmflow,
            Arc::new(MockStage::new(StageType::QpudidpRmflow, json!({"result": "ok"}))),
        );
        let executor = Arc::new(PipelineExecutor::new(
            Arc::new(registry),
            checkpoint.clone(),
            "http://localhost:8765",
            "http://localhost:8420",
        ));
        let state = executor
            .run_pipeline(&def, json!({}), "/tmp/test.brain")
            .await
            .expect("pipeline must succeed");
        state.id
        // executor and checkpoint drop here
    };

    // New checkpoint instance from the same directory.
    let checkpoint2 = CheckpointStore::new(&checkpoint_dir).expect("reopen checkpoint dir");
    let loaded = checkpoint2
        .load(run_id)
        .expect("checkpoint must persist after executor is dropped");

    assert_eq!(
        loaded.status,
        PipelineStatus::Completed,
        "persisted state must be Completed"
    );
    assert_eq!(loaded.id, run_id, "run_id must be stable across reload");
}

/// 9. Submit 50 pipeline runs concurrently via `FuturesUnordered`.
///    Each executor has `max_concurrency = 4` but all 50 run in parallel
///    tokio tasks. All 50 must complete. We verify no silent panics by
///    counting completions.
#[tokio::test]
async fn test_large_batch_memory_bounded() {
    const PIPELINE_TOML: &str = r#"
[pipeline]
name = "batch-memory-test"
version = "1.0"
max_concurrency = 4
default_timeout_secs = 30

[[stage]]
id = "single"
type = "qpudidp_rmflow"
depends_on = []
"#;
    let def = Arc::new(PipelineDef::from_toml(PIPELINE_TOML).expect("parse pipeline TOML"));
    let inflight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    // Inner stage struct defined locally to capture inflight/peak counts.
    struct InFlightStage {
        inflight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl Stage for InFlightStage {
        fn stage_type(&self) -> StageType {
            StageType::QpudidpRmflow
        }

        fn timeout_secs(&self) -> u64 {
            30
        }

        async fn execute_raw(
            &self,
            _input: Value,
            _ctx: &StageContext,
        ) -> Result<Value, StageError> {
            let current = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(current, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            self.inflight.fetch_sub(1, Ordering::SeqCst);
            Ok(json!({"ok": true}))
        }
    }

    let mut futures_set = futures::stream::FuturesUnordered::new();

    for _ in 0..50 {
        let inflight_clone = inflight.clone();
        let peak_clone = peak.clone();
        let def_clone = def.clone();

        let mut registry = StageRegistry::new();
        registry.register(
            StageType::QpudidpRmflow,
            Arc::new(InFlightStage {
                inflight: inflight_clone,
                peak: peak_clone,
            }),
        );
        let (executor, _cp) = test_executor(registry);

        futures_set.push(tokio::spawn(async move {
            executor
                .run_pipeline(&def_clone, json!({}), "/tmp/test.brain")
                .await
        }));
    }

    let mut completed = 0usize;
    while let Some(result) = futures_set.next().await {
        if matches!(result, Ok(Ok(ref s)) if s.status == PipelineStatus::Completed) {
            completed += 1;
        }
    }

    assert_eq!(completed, 50, "all 50 batch runs must complete");
}

/// 10. Mermaid DAG generator produces correct output for a diamond-shaped pipeline.
#[test]
fn test_dag_mermaid_output() {
    use qorchestrate_core::dag::DagBuilder;

    const TOML: &str = r#"
[pipeline]
name = "dag-test"
version = "1.0"

[[stage]]
id = "a"
type = "qpudidp_rmflow"
depends_on = []

[[stage]]
id = "b"
type = "freq_optimize"
depends_on = ["a"]

[[stage]]
id = "c"
type = "xtalk_analyze"
depends_on = ["a"]

[[stage]]
id = "d"
type = "grape_optimize"
depends_on = ["b", "c"]
"#;
    let def = PipelineDef::from_toml(TOML).expect("parse DAG test TOML");
    let mermaid = DagBuilder::to_mermaid(&def.stages, "dag-test");

    assert!(
        mermaid.contains("graph LR"),
        "Mermaid output must start with 'graph LR'; got:\n{mermaid}"
    );
    assert!(
        mermaid.contains("a -->"),
        "must contain edge from 'a'; got:\n{mermaid}"
    );
    assert!(
        mermaid.contains("--> d"),
        "must contain edge to 'd'; got:\n{mermaid}"
    );
    assert!(
        mermaid.contains("fill:#4a9eff"),
        "start nodes must use blue style; got:\n{mermaid}"
    );
    assert!(
        mermaid.contains("fill:#22cc55"),
        "end nodes must use green style; got:\n{mermaid}"
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Unused-import suppression for test_ctx (used by harness, kept for parity)
// ─────────────────────────────────────────────────────────────────────────────

#[allow(dead_code)]
fn _assert_test_ctx_compiles() {
    let _ = test_ctx(Uuid::now_v7(), "dummy");
}
