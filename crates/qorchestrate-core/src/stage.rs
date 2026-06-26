use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use uuid::Uuid;

use crate::errors::StageError;
use crate::event::StageEvent;

/// Context passed to every stage execution.
#[derive(Clone)]
pub struct StageContext {
    pub pipeline_run_id: Uuid,
    pub stage_id: String,
    pub quantum_api_url: String,
    pub qpudidp_url: String,
    pub brain_path: PathBuf,
    pub event_tx: broadcast::Sender<StageEvent>,
    pub nesting_depth: usize,
}

impl StageContext {
    pub fn new(
        pipeline_run_id: Uuid,
        stage_id: impl Into<String>,
        quantum_api_url: impl Into<String>,
        qpudidp_url: impl Into<String>,
        brain_path: PathBuf,
        event_tx: broadcast::Sender<StageEvent>,
    ) -> Self {
        Self {
            pipeline_run_id,
            stage_id: stage_id.into(),
            quantum_api_url: quantum_api_url.into(),
            qpudidp_url: qpudidp_url.into(),
            brain_path,
            event_tx,
            nesting_depth: 0,
        }
    }

    /// Create a child context for a sub-stage, inheriting all connection settings.
    pub fn child(&self, stage_id: impl Into<String>) -> Self {
        Self {
            pipeline_run_id: self.pipeline_run_id,
            stage_id: stage_id.into(),
            quantum_api_url: self.quantum_api_url.clone(),
            qpudidp_url: self.qpudidp_url.clone(),
            brain_path: self.brain_path.clone(),
            event_tx: self.event_tx.clone(),
            nesting_depth: self.nesting_depth,
        }
    }

    /// Create a nested context for a sub-pipeline call, incrementing nesting depth.
    pub fn nested(&self, pipeline_run_id: Uuid) -> Self {
        Self {
            pipeline_run_id,
            nesting_depth: self.nesting_depth + 1,
            ..self.clone()
        }
    }
}

/// All recognized stage variants across the quantum design pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageType {
    QpudidpRmflow,
    QpudidpCmaes,
    QemSolve,
    ScqSimulate,
    FreqOptimize,
    XtalkAnalyze,
    ReadoutDesign,
    GrapeOptimize,
    DragOptimize,
    PqecAssess,
    QecThreshold,
    /// Cross-platform QEC compiler: modality → recommended code/distance/decoder.
    QecCompile,
    TwinCompare,
    TwinRecalibrate,
    TwinQecUpdate,
    TwinMock,
    /// Ingest a raw cryo-measurement record → compare against the design →
    /// digital twin + recalibration (metrology ingestion, post-fab loop).
    MetrologyIngest,
    /// Acquire a (simulated-instrument) characterization of a design → compare
    /// → twin + recalibration (closes the loop without a hand-supplied record).
    MetrologyAcquire,
    /// Consume the twin's `TriggerInverseDesign` recalibration hints and re-invoke
    /// QPUDIDP inverse design *seeded by each hint* (targeted re-design — the loop
    /// closer, vs a blanket whole-chip rerun).
    RecalDispatch,
    BenchPredict,
    QexplorePareto,
    FreqYield,
    SurgeryResources,
    OqfpBuild,
    OqfpValidate,
    // ── Fabrication handoff (claw-gds via quantum-api) ─────────────────────
    /// Generate a GDS-II chip layout from upstream device + frequency-plan
    /// output by calling the quantum-api `/gds/*` endpoints. Emits the layout
    /// bytes (hex) plus metadata as an artifact for tape-out.
    GdsGenerate,
    /// Run a design-rule check over the generated GDS layout. Fails the stage
    /// when violations are found so a non-manufacturable layout blocks the run.
    DrcCheck,
    /// Assemble the fabrication-handoff bundle (GDS-II + DRC report + validated
    /// OQFP spec + signed manifest) into a submission directory for a foundry.
    TapeoutPackage,
    /// Evaluate the Josephson-junction process recipe → nominal I_c / E_J / L_J
    /// + process parameters, for the OQFP device + fabrication layers.
    ProcessRecipe,
    Clawhdf5Save,
    PipelineCall,
    Batch,
    Skip,
    BayesianOuterLoop,
    FailureModeAnalysis,
    /// Generic aggregator: returns its full input map (including the
    /// `<dep_id>_output` keys the executor populates from dependencies)
    /// as this stage's output. Useful as a terminal stage in templates that
    /// fan out N parallel sub-pipelines and want one structured result.
    CollectDeps,
    /// Generic HTTP POST against the quantum-api gateway. Reads the target
    /// path from the input's `path` field and POSTs the remaining input
    /// as the JSON body. Lets templates reach any quantum-api endpoint
    /// without needing a per-endpoint stage struct.
    HttpPost,
    // ── Parametric process design (rustyqcirc) ─────────────────────────────
    QcircQuantize,
    QcircProcesses,
    QcircPumpDesign,
    QcircFloquet,
    QcircRegimeScan,
    QcircConstraints,
    QcircSummary,
    // Non-SC modality stages (Qion*, Qatom*, Qspin*) were removed when the
    // generic `http_post` stage subsumed their one-shot POST behaviour.
    // Templates now declare `type = "http_post", params.path = "/q<mod>/<op>"`.
}

/// Trait implemented by every executable stage.
///
/// Both input and output use `serde_json::Value` at the trait-object boundary.
/// Concrete implementations deserialize from / serialize to their typed structs
/// internally, keeping the executor generic.
#[async_trait]
pub trait Stage: Send + Sync {
    async fn execute_raw(
        &self,
        input: serde_json::Value,
        ctx: &StageContext,
    ) -> Result<serde_json::Value, StageError>;

    fn stage_type(&self) -> StageType;

    fn timeout_secs(&self) -> u64 {
        60
    }
}
