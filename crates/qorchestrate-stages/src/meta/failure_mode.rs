//! Failure-Mode & Effects Analysis (FMEA) stage — self-contained.
//!
//! Was a phantom: `StageType::FailureModeAnalysis` existed with no impl. This
//! inspects the accumulated pipeline output (twin recal actions, fidelity,
//! frequency collisions, yield) and emits a ranked list of failure modes with a
//! severity score, so a pipeline can gate or report on design risk.

use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct FailureModeAnalysisStage;

impl FailureModeAnalysisStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for FailureModeAnalysisStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Recursively find the first numeric value for `key` anywhere in the tree.
fn find_number(v: &Value, key: &str) -> Option<f64> {
    match v {
        Value::Object(m) => {
            if let Some(n) = m.get(key).and_then(|x| x.as_f64()) {
                return Some(n);
            }
            m.values().find_map(|c| find_number(c, key))
        }
        Value::Array(a) => a.iter().find_map(|c| find_number(c, key)),
        _ => None,
    }
}

/// Recursively find the first boolean value for `key`.
fn find_bool(v: &Value, key: &str) -> Option<bool> {
    match v {
        Value::Object(m) => {
            if let Some(b) = m.get(key).and_then(|x| x.as_bool()) {
                return Some(b);
            }
            m.values().find_map(|c| find_bool(c, key))
        }
        Value::Array(a) => a.iter().find_map(|c| find_bool(c, key)),
        _ => None,
    }
}

/// Count occurrences of an object key anywhere in the tree (e.g. recal-action tags).
fn count_key(v: &Value, key: &str) -> u64 {
    match v {
        Value::Object(m) => {
            let here = m.contains_key(key) as u64;
            here + m.values().map(|c| count_key(c, key)).sum::<u64>()
        }
        Value::Array(a) => a.iter().map(|c| count_key(c, key)).sum(),
        _ => 0,
    }
}

/// Produce a ranked failure-mode report from the pipeline state.
pub fn analyze_failure_modes(input: &Value) -> Value {
    let mut modes: Vec<(String, f64, String)> = Vec::new(); // (mode, severity 1-10, detail)

    if let Some(c) = find_number(input, "critical_count")
        && c > 0.0
    {
        modes.push(("qubit_failure".into(), 9.0, format!("{c} critical qubit(s)")));
    }
    let n_redesign = count_key(input, "TriggerInverseDesign");
    if n_redesign > 0 {
        modes.push(("requires_redesign".into(), 8.0, format!("{n_redesign} qubit(s) need re-design")));
    }
    if count_key(input, "FlagForReplacement") > 0 {
        modes.push(("uncorrectable_defect".into(), 10.0, "flagged for replacement".into()));
    }
    if let Some(f) = find_number(input, "gate_fidelity")
        && f < 0.99
    {
        modes.push(("low_gate_fidelity".into(), (1.0 - f) * 200.0, format!("gate fidelity {f:.4}")));
    }
    if find_bool(input, "collision_free") == Some(false) {
        modes.push(("frequency_collision".into(), 7.0, "frequency plan not collision-free".into()));
    }
    if find_bool(input, "meets_threshold") == Some(false) {
        modes.push(("below_qec_threshold".into(), 8.0, "QEC threshold not met".into()));
    }
    if let Some(y) = find_number(input, "yield_estimate")
        && y < 0.5
    {
        modes.push(("low_yield".into(), (1.0 - y) * 10.0, format!("yield {y:.2}")));
    }

    modes.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let top = modes.first().map(|m| m.0.clone());
    let list: Vec<Value> = modes
        .iter()
        .map(|(mode, sev, detail)| json!({ "mode": mode, "severity": (sev * 10.0).round() / 10.0, "detail": detail }))
        .collect();

    json!({
        "n_failure_modes": list.len(),
        "top_risk": top,
        "passes": list.is_empty(),
        "failure_modes": list,
    })
}

#[async_trait]
impl Stage for FailureModeAnalysisStage {
    fn stage_type(&self) -> StageType {
        StageType::FailureModeAnalysis
    }

    fn timeout_secs(&self) -> u64 {
        5
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        Ok(analyze_failure_modes(&input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks_critical_failures_highest() {
        let input = json!({
            "calibration_phase_output": { "recal_actions": { "critical_count": 2,
                "suggestions": [{ "action": { "FlagForReplacement": null } },
                                { "action": { "TriggerInverseDesign": { "hint": {} } } }] } },
            "qec_assess_output": { "meets_threshold": false },
        });
        let report = analyze_failure_modes(&input);
        assert_eq!(report.get("passes").unwrap(), &json!(false));
        // uncorrectable_defect (severity 10) ranks first
        assert_eq!(report.get("top_risk").unwrap(), &json!("uncorrectable_defect"));
        assert!(report.get("n_failure_modes").unwrap().as_u64().unwrap() >= 3);
    }

    #[test]
    fn clean_design_passes() {
        let input = json!({ "freq_plan_output": { "collision_free": true, "yield_estimate": 0.85 } });
        let report = analyze_failure_modes(&input);
        assert_eq!(report.get("passes").unwrap(), &json!(true));
        assert_eq!(report.get("n_failure_modes").unwrap(), &json!(0));
    }
}
