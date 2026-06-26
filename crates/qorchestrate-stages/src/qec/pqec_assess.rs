use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct PqecAssessStage {
    client: Client,
}

impl PqecAssessStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for PqecAssessStage {
    fn default() -> Self {
        Self::new()
    }
}

/// A JSON value that is either a number or an array of numbers → a scalar.
fn avg_or_scalar(v: &Value) -> Option<f64> {
    if let Some(n) = v.as_f64() {
        return Some(n);
    }
    let nums: Vec<f64> = v.as_array()?.iter().filter_map(|x| x.as_f64()).collect();
    (!nums.is_empty()).then(|| nums.iter().sum::<f64>() / nums.len() as f64)
}

/// Resolve a numeric field: an explicit top-level value wins; otherwise look
/// through the listed `(upstream_output_key, candidate_field_names)` sources.
fn resolve(input: &Value, direct: &str, sources: &[(&str, &[&str])]) -> Option<f64> {
    if let Some(v) = input.get(direct).and_then(|v| v.as_f64()) {
        return Some(v);
    }
    for (src, keys) in sources {
        if let Some(n) = input.get(src) {
            if let Some(v) = keys.iter().find_map(|k| n.get(*k)).and_then(avg_or_scalar) {
                return Some(v);
            }
        }
    }
    None
}

/// Build the `/pqec/assess` request from upstream stage outputs.
///
/// The endpoint reads top-level `gate_fidelity` / `gate_time` / `t1` / `t2`; the
/// raw pipeline blob nests those inside `*_output` keys, so forwarding it
/// unchanged makes the endpoint silently fall back to its 0.999 / 80 µs / 60 µs
/// defaults. This pulls the *real* optimized gate fidelity from the pulse stage
/// (falling back to the crosstalk worst-case if pulse optimization was skipped)
/// and the coherence from the device-sim stage.
pub fn build_assess_request(input: &Value) -> Value {
    let mut req = serde_json::Map::new();
    let fields: &[(&str, &[(&str, &[&str])])] = &[
        (
            "gate_fidelity",
            &[
                ("pulse_optimize_output", &["fidelity", "gate_fidelity"]),
                ("xtalk_analyze_output", &["worst_case_gate_fidelity"]),
            ],
        ),
        (
            "gate_time",
            &[("pulse_optimize_output", &["duration_ns", "gate_time"])],
        ),
        ("t1", &[("scq_device_output", &["t1_us"])]),
        ("t2", &[("scq_device_output", &["t2_us"])]),
    ];
    for (name, sources) in fields {
        if let Some(v) = resolve(input, name, sources) {
            req.insert((*name).to_string(), json!(v));
        }
    }
    Value::Object(req)
}

#[async_trait]
impl Stage for PqecAssessStage {
    fn stage_type(&self) -> StageType {
        StageType::PqecAssess
    }

    fn timeout_secs(&self) -> u64 {
        60
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let body = build_assess_request(&input);
        let resp = self
            .client
            .post(format!("{}/pqec/assess", ctx.quantum_api_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        resp.json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_real_pulse_fidelity_not_default() {
        let input = json!({
            "pulse_optimize_output": { "fidelity": 0.9962, "duration_ns": 35.0 },
            "scq_device_output": { "t1_us": [90.0, 110.0], "t2_us": [70.0, 80.0] },
        });
        let req = build_assess_request(&input);
        assert_eq!(req.get("gate_fidelity").unwrap(), &json!(0.9962));
        assert_eq!(req.get("gate_time").unwrap(), &json!(35.0));
        assert_eq!(req.get("t1").unwrap(), &json!(100.0)); // avg(90, 110)
        assert_eq!(req.get("t2").unwrap(), &json!(75.0));
    }

    #[test]
    fn falls_back_to_crosstalk_when_pulse_skipped() {
        // pulse_optimize is conditional; if skipped, use the crosstalk worst-case.
        let input = json!({
            "xtalk_analyze_output": { "worst_case_gate_fidelity": 0.9991 },
        });
        let req = build_assess_request(&input);
        assert_eq!(req.get("gate_fidelity").unwrap(), &json!(0.9991));
        // no coherence available → omitted so the endpoint applies its defaults
        assert!(req.get("t1").is_none());
    }

    #[test]
    fn explicit_value_overrides_upstream() {
        let input = json!({
            "gate_fidelity": 0.95,
            "pulse_optimize_output": { "fidelity": 0.999 },
        });
        assert_eq!(build_assess_request(&input).get("gate_fidelity").unwrap(), &json!(0.95));
    }
}
