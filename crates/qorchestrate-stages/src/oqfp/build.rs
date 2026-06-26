use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Average a JSON value that is either a number or an array of numbers.
fn avg_num(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let nums: Vec<f64> = v.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
    if nums.is_empty() {
        None
    } else {
        Some(nums.iter().sum::<f64>() / nums.len() as f64)
    }
}

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
        let gds = input
            .get("gds_generate_output")
            .cloned()
            .unwrap_or(Value::Null);
        let drc = input
            .get("drc_check_output")
            .cloned()
            .unwrap_or(Value::Null);
        let recipe_out = input
            .get("process_recipe_output")
            .cloned()
            .unwrap_or(Value::Null);
        let jeval = recipe_out.get("eval").cloned().unwrap_or(Value::Null);

        // ── derive control / wiring / performance values from upstream outputs ──
        let n_qubits = gds
            .get("num_qubits")
            .and_then(|v| v.as_u64())
            .or_else(|| scq.get("t1_us").and_then(|v| v.as_array()).map(|a| a.len() as u64))
            .unwrap_or(4);
        let avg_t1 = scq.get("t1_us").and_then(avg_num);
        let avg_t2 = scq.get("t2_us").and_then(avg_num);
        let gate_fid_1q = pulse.get("fidelity").and_then(|v| v.as_f64());
        let gate_dur_1q = pulse.get("duration_ns").and_then(|v| v.as_f64()).unwrap_or(30.0);
        let pulse_shape = pulse
            .get("pulse_shape")
            .and_then(|v| v.as_str())
            .unwrap_or("DRAG")
            .to_string();
        let gate_fid_2q = xtalk
            .get("cr_gate")
            .and_then(|c| c.get("fidelity"))
            .and_then(|v| v.as_f64())
            .or_else(|| gate_fid_1q.map(|f| f * f)) // rough 2q ≈ 1q²
            .unwrap_or(0.99);
        let readout_fid = readout
            .get("readout_fidelity")
            .and_then(|v| v.as_f64())
            .or_else(|| {
                readout
                    .get("metrics")
                    .and_then(|m| m.get("assignment_fidelity"))
                    .and_then(|v| v.as_f64())
            })
            .unwrap_or(0.99);
        let first_freq = freq_plan
            .get("assignments")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .and_then(|q| q.get("frequency_ghz").or_else(|| q.get("freq_ghz")))
            .and_then(|v| v.as_f64())
            .unwrap_or(5.0);
        let fridge_model = input
            .get("fridge_model")
            .and_then(|v| v.as_str())
            .unwrap_or("Bluefors LD400")
            .to_string();

        let oqfp = json!({
            "oqfp_version": "1.0",
            "layers": {
                "device": {
                    "geometry": inverse.get("best_candidate").and_then(|c| c.get("geometry")).cloned().unwrap_or(Value::Null),
                    "scq_params": scq,
                    "junction": {
                        "lj_nh": jeval.get("lj_nh").cloned().unwrap_or(Value::Null),
                        "ic_ua": jeval.get("ic_ua").cloned().unwrap_or(Value::Null),
                        "area_um2": jeval.get("area_um2").cloned().unwrap_or(Value::Null),
                    },
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
                    "native_gates": ["id", "rz", "sx", "x", "cz"],
                    "pulse_shape": pulse_shape,
                    "gate_fidelity": gate_fid_1q,
                    "gate_library": [
                        {"name": "x",  "qubits": [], "pulse_shape": pulse_shape, "duration_ns": gate_dur_1q, "fidelity": gate_fid_1q},
                        {"name": "sx", "qubits": [], "pulse_shape": pulse_shape, "duration_ns": gate_dur_1q, "fidelity": gate_fid_1q},
                        {"name": "cz", "qubits": [], "pulse_shape": "GaussianSquare", "duration_ns": gate_dur_1q * 4.0, "fidelity": gate_fid_2q},
                    ],
                    "calibration_targets": [
                        {"parameter": "qubit_frequency_ghz", "target_value": first_freq, "tolerance": 0.005},
                        {"parameter": "t1_us", "target_value": avg_t1, "tolerance": 10.0},
                        {"parameter": "t2_us", "target_value": avg_t2, "tolerance": 10.0},
                        {"parameter": "readout_fidelity", "target_value": readout_fid, "tolerance": 0.005},
                    ],
                    "readout": readout,
                },
                "fabrication": {
                    "yield_estimate": freq_plan.get("yield_estimate").cloned().unwrap_or(json!(0.0)),
                    "process_params": {
                        "fab_process": input.get("fab_process").cloned().unwrap_or(json!("AlOx_0.5um")),
                    },
                    "junction_recipe": recipe_out.get("recipe").and_then(|r| r.get("name")).cloned().unwrap_or(Value::Null),
                    "junction_sigma_percent": jeval.get("junction_sigma_percent").cloned().unwrap_or(Value::Null),
                    "gds_file": gds.get("lib_name").cloned().unwrap_or(Value::Null),
                    "gds_n_bytes": gds.get("n_bytes").cloned().unwrap_or(Value::Null),
                    "num_qubits": gds.get("num_qubits").cloned().unwrap_or(Value::Null),
                    "drc_clean": drc.get("clean").cloned().unwrap_or(Value::Null),
                    "drc_num_violations": drc.get("num_violations").cloned().unwrap_or(Value::Null),
                    "fracture": gds.get("fracture").cloned().unwrap_or(Value::Null),
                    "wiring": {
                        // Cryostat harness: a drive + readout + flux line per qubit.
                        "fridge_model": fridge_model,
                        "signal_lines": n_qubits * 3,
                        "thermal_budget_ok": n_qubits <= 1000,
                    },
                },
                "performance": {
                    "quantum_volume": bench.get("quantum_volume").cloned().unwrap_or(Value::Null),
                    "clops": bench.get("clops").cloned().unwrap_or(Value::Null),
                    "logical_error_rate": qec.get("logical_error_rate").cloned().unwrap_or(Value::Null),
                    "avg_t1_us": avg_t1,
                    "avg_t2_us": avg_t2,
                    "avg_2q_fidelity": gate_fid_2q,
                    "avg_readout_fidelity": readout_fid,
                },
                "application": {
                    "target_use_case": input.get("target_use_case").cloned().unwrap_or(json!("general_purpose")),
                    "logical_qubits_required": input.get("logical_qubits_required").cloned().unwrap_or(Value::Null),
                }
            }
        });

        Ok(json!({ "oqfp_spec": oqfp, "validated": false }))
    }
}
