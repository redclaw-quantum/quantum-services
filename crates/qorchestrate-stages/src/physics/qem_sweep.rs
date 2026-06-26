//! Driven-modal frequency sweep stage: geometry → S-parameters (the HFSS
//! driven-modal equivalent). Feeds `BbqQuantize` to close the
//! geometry → S-params → Hamiltonian path that previously required a manual
//! hand-off between the qem-server and rustybbq.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct QemSweepStage {
    client: Client,
}

impl QemSweepStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for QemSweepStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Geometry field names for the TransmonCross `FrequencySweepRequest`, in the
/// order QPUDIDP emits its 7-D geometry vector.
const GEOM_FIELDS: [(&str, f64); 7] = [
    ("cross_length", 0.3),
    ("cross_width", 0.02),
    ("cross_gap", 0.02),
    ("claw_length", 0.1),
    ("claw_width", 0.01),
    ("claw_gap", 0.006),
    ("lj_nh", 12.0),
];

/// Build a `/qem/sweep` (`FrequencySweepRequest`) body from the pipeline input:
/// named geometry fields win, else the inverse-design best-candidate 7-D vector,
/// else physical defaults; the sweep range falls back to a 4–8 GHz band.
pub fn build_sweep_request(input: &Value) -> Value {
    let params = input
        .get("inverse_design_output")
        .and_then(|d| d.get("best_candidate"))
        .and_then(|c| c.get("geometry"))
        .and_then(|g| g.get("params"))
        .and_then(|p| p.as_array());

    let mut body = serde_json::Map::new();
    for (i, (name, default)) in GEOM_FIELDS.iter().enumerate() {
        let v = input
            .get(name)
            .and_then(|v| v.as_f64())
            .or_else(|| params.and_then(|a| a.get(i)).and_then(|v| v.as_f64()))
            .unwrap_or(*default);
        body.insert((*name).to_string(), json!(v));
    }
    let f = |name: &str, default: f64| input.get(name).and_then(|v| v.as_f64()).unwrap_or(default);
    body.insert("f_start_ghz".into(), json!(f("f_start_ghz", 4.0)));
    body.insert("f_stop_ghz".into(), json!(f("f_stop_ghz", 8.0)));
    if let Some(n) = input.get("n_points").and_then(|v| v.as_u64()) {
        body.insert("n_points".into(), json!(n));
    }
    if let Some(s) = input.get("substrate").and_then(|v| v.as_str()) {
        body.insert("substrate".into(), json!(s));
    }
    Value::Object(body)
}

#[async_trait]
impl Stage for QemSweepStage {
    fn stage_type(&self) -> StageType {
        StageType::QemSweep
    }

    fn timeout_secs(&self) -> u64 {
        120
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let body = build_sweep_request(&input);
        let resp = self
            .client
            .post(format!("{}/qem/sweep", ctx.quantum_api_url))
            .json(&body)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, b)));
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
    fn maps_inverse_design_geometry_into_sweep_request() {
        let input = json!({
            "inverse_design_output": { "best_candidate": { "geometry": {
                "params": [0.31, 0.021, 0.019, 0.11, 0.011, 0.0061, 11.5]
            }}},
            "f_start_ghz": 5.0, "f_stop_ghz": 7.0,
        });
        let req = build_sweep_request(&input);
        assert_eq!(req.get("cross_length").unwrap(), &json!(0.31));
        assert_eq!(req.get("lj_nh").unwrap(), &json!(11.5));
        assert_eq!(req.get("f_start_ghz").unwrap(), &json!(5.0));
        assert_eq!(req.get("f_stop_ghz").unwrap(), &json!(7.0));
    }

    #[test]
    fn falls_back_to_defaults_without_geometry() {
        let req = build_sweep_request(&json!({}));
        // all 6 geometry fields + lj + sweep band present so the request is valid
        for (name, _) in super::GEOM_FIELDS {
            assert!(req.get(name).is_some(), "missing {name}");
        }
        assert_eq!(req.get("f_start_ghz").unwrap(), &json!(4.0));
    }
}
