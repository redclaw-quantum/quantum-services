use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// In-process assembly stage — merges outputs from all upstream parametric
/// process stages into a single `ParametricProcessReport`.
///
/// Collects:
/// - `qcirc_processes_output`  → process list (3WM/4WM/Kerr coupling strengths)
/// - `qcirc_pump_design_output` → best pump spec (type, freq, amplitude)
/// - `qcirc_floquet_output`    → Floquet quasi-energies, collision map
/// - `qcirc_regime_scan_output` → Pareto-optimal regime points
/// - `qcirc_constraints_output` → circuit parameter constraints
pub struct QcircSummaryStage;

impl QcircSummaryStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for QcircSummaryStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QcircSummaryStage {
    fn stage_type(&self) -> StageType {
        StageType::QcircSummary
    }

    fn timeout_secs(&self) -> u64 {
        5
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        let processes = input
            .get("qcirc_processes_output")
            .cloned()
            .unwrap_or(Value::Null);

        let pump = input
            .get("qcirc_pump_design_output")
            .cloned()
            .unwrap_or(Value::Null);

        let floquet = input
            .get("qcirc_floquet_output")
            .cloned()
            .unwrap_or(Value::Null);

        let regime = input
            .get("qcirc_regime_scan_output")
            .cloned()
            .unwrap_or(Value::Null);

        let constraints = input
            .get("qcirc_constraints_output")
            .cloned()
            .unwrap_or(Value::Null);

        // Extract key summary metrics for quick inspection
        let dominant_process = processes
            .get("processes")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|p| p.get("process_type"))
            .cloned()
            .unwrap_or(json!("unknown"));

        let n_processes = processes
            .get("processes")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        let pump_freq_ghz = pump
            .get("pump_frequency_ghz")
            .cloned()
            .unwrap_or(json!(null));

        let pump_type = pump
            .get("pump_type")
            .cloned()
            .unwrap_or(json!(null));

        let n_collisions = floquet
            .get("n_collisions")
            .cloned()
            .unwrap_or(json!(0));

        let collision_free = floquet
            .get("collision_free")
            .cloned()
            .unwrap_or(json!(null));

        let n_pareto_points = regime
            .get("pareto_front")
            .and_then(|v| v.as_array())
            .map(|a| a.len())
            .unwrap_or(0);

        Ok(json!({
            "summary": {
                "dominant_process": dominant_process,
                "n_processes_identified": n_processes,
                "pump_type": pump_type,
                "pump_frequency_ghz": pump_freq_ghz,
                "floquet_collision_free": collision_free,
                "n_floquet_collisions": n_collisions,
                "n_pareto_regime_points": n_pareto_points,
            },
            "processes": processes,
            "pump_design": pump,
            "floquet": floquet,
            "regime_scan": regime,
            "circuit_constraints": constraints,
        }))
    }
}
