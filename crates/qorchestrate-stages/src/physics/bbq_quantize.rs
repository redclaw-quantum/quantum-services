//! Black-Box Quantization stage: S-parameters → Hamiltonian (E_J/E_C/g/χ) via
//! Foster synthesis. Consumes `QemSweep`'s `s_parameters` directly — the
//! qem-driven `SParameters` shape is identical to bbq's `BbqSParams`, so the
//! "adapter" is a field extraction, no conversion.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct BbqQuantizeStage {
    client: Client,
}

impl BbqQuantizeStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for BbqQuantizeStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `/bbq/quantize` body: the S-parameter object from the upstream
/// sweep (or an explicit `s_params`), with optional `ec`/`n_poles` merged in
/// (the endpoint reads those alongside the S-params and defaults them otherwise).
pub fn build_quantize_request(input: &Value) -> Option<Value> {
    let mut sparams = input
        .get("qem_sweep_output")
        .and_then(|s| s.get("s_parameters"))
        .or_else(|| input.get("s_params"))
        .or_else(|| input.get("s_parameters"))
        .cloned()?;

    if let Value::Object(ref mut m) = sparams {
        for k in ["ec", "n_poles", "dw_ghz"] {
            if let Some(v) = input.get(k) {
                m.insert(k.to_string(), v.clone());
            }
        }
    }
    Some(sparams)
}

#[async_trait]
impl Stage for BbqQuantizeStage {
    fn stage_type(&self) -> StageType {
        StageType::BbqQuantize
    }

    fn timeout_secs(&self) -> u64 {
        60
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let body = build_quantize_request(&input).ok_or_else(|| {
            StageError::BackendError(
                "bbq_quantize: no S-parameters found (expected qem_sweep_output.s_parameters or s_params)".into(),
            )
        })?;
        let resp = self
            .client
            .post(format!("{}/bbq/quantize", ctx.quantum_api_url))
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
    use serde_json::json;

    #[test]
    fn extracts_sweep_s_parameters() {
        let input = json!({
            "qem_sweep_output": {
                "s_parameters": { "frequencies_ghz": [5.0, 6.0], "data": [], "port_count": 1, "z_ref": 50.0 },
                "solver": "driven"
            }
        });
        let body = build_quantize_request(&input).unwrap();
        assert_eq!(body.get("port_count").unwrap(), &json!(1));
        assert_eq!(body.get("z_ref").unwrap(), &json!(50.0));
    }

    #[test]
    fn merges_optional_ec_and_n_poles() {
        let input = json!({
            "s_params": { "frequencies_ghz": [5.0], "data": [], "port_count": 1, "z_ref": 50.0 },
            "ec": [0.25], "n_poles": 8
        });
        let body = build_quantize_request(&input).unwrap();
        assert_eq!(body.get("ec").unwrap(), &json!([0.25]));
        assert_eq!(body.get("n_poles").unwrap(), &json!(8));
    }

    #[test]
    fn none_without_s_parameters() {
        assert!(build_quantize_request(&json!({ "foo": 1 })).is_none());
    }
}
