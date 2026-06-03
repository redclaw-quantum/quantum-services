use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct OqfpBuildStage;

impl OqfpBuildStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OqfpBuildStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for OqfpBuildStage {
    fn stage_type(&self) -> StageType {
        StageType::OqfpBuild
    }

    fn timeout_secs(&self) -> u64 {
        10
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        let freq_plan = input
            .get("freq_plan_output")
            .cloned()
            .unwrap_or(Value::Null);
        let xtalk = input
            .get("xtalk_analyze_output")
            .cloned()
            .unwrap_or(Value::Null);
        let readout = input
            .get("readout_design_output")
            .cloned()
            .unwrap_or(Value::Null);
        let pulse = input
            .get("pulse_optimize_output")
            .cloned()
            .unwrap_or(Value::Null);
        let qec = input
            .get("qec_assess_output")
            .cloned()
            .unwrap_or(Value::Null);
        let bench = input
            .get("bench_predict_output")
            .cloned()
            .unwrap_or(Value::Null);
        let inverse = input
            .get("inverse_design_output")
            .cloned()
            .unwrap_or(Value::Null);
        let scq = input
            .get("scq_device_output")
            .cloned()
            .unwrap_or(Value::Null);

        let oqfp = json!({
            "oqfp_version": "1.0",
            "layers": {
                "device": {
                    "geometry": inverse.get("best_candidate").and_then(|c| c.get("geometry")).cloned().unwrap_or(Value::Null),
                    "scq_params": scq,
                },
                "connectivity": {
                    "edges": xtalk.get("coupling").and_then(|c| c.get("coupling_matrix_mhz")).cloned().unwrap_or(json!([])),
                },
                "frequency": {
                    "assignments": freq_plan.get("assignments").cloned().unwrap_or(json!([])),
                    "collision_free": freq_plan.get("collision_free").cloned().unwrap_or(json!(false)),
                },
                "qec": {
                    "meets_threshold": qec.get("meets_threshold").cloned().unwrap_or(json!(false)),
                    "recommended_code": qec.get("recommended_code").cloned().unwrap_or(json!("surface_code")),
                    "max_achievable_distance": qec.get("max_achievable_distance").cloned().unwrap_or(json!(3)),
                },
                "control": {
                    "pulse_shape": pulse.get("pulse_shape").cloned().unwrap_or(json!("DRAG")),
                    "gate_fidelity": pulse.get("fidelity").cloned().unwrap_or(Value::Null),
                    "readout": readout,
                },
                "fabrication": {
                    "yield_estimate": freq_plan.get("yield_estimate").cloned().unwrap_or(json!(0.0)),
                },
                "performance": {
                    "quantum_volume": bench.get("quantum_volume").cloned().unwrap_or(Value::Null),
                    "clops": bench.get("clops").cloned().unwrap_or(Value::Null),
                    "logical_error_rate": qec.get("logical_error_rate").cloned().unwrap_or(Value::Null),
                },
                "application": {}
            }
        });

        Ok(json!({ "oqfp_spec": oqfp, "validated": false }))
    }
}
