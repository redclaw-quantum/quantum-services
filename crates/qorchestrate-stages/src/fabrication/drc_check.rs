use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Run a design-rule check over a generated chip layout.
///
/// Reads the `chip_params` echoed by the upstream `gds_generate` stage and POSTs
/// them to the quantum-api `/drc` endpoint, which rebuilds the same layout and
/// checks it. The DRC report (`clean`, `num_violations`, `violations`) is
/// returned as the stage output.
///
/// By default the stage is **report-only**: it surfaces violations without
/// failing, because the default rule set flags intentionally-abutting transmon
/// gap polygons. Set `fail_on_violations = true` (stage param) to gate the
/// pipeline on a clean layout.
pub struct DrcCheckStage {
    client: Client,
}

impl DrcCheckStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for DrcCheckStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the chip-layout request to check. Prefers the params the
/// `gds_generate` stage actually used (so the checked layout matches the
/// exported one); otherwise falls back to any explicit params on this stage.
fn chip_params_for_drc(input: &Value) -> Value {
    if let Some(p) = input
        .get("gds_generate_output")
        .and_then(|g| g.get("chip_params"))
    {
        return p.clone();
    }
    let mut params = json!({});
    let obj = params.as_object_mut().expect("params is an object");
    for key in ["cols", "rows", "pitch_x", "pitch_y", "qubit_params"] {
        if let Some(v) = input.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }
    params
}

/// Pure gating decision, factored out so it can be unit-tested without a live
/// quantum-api. Returns an error only when gating is requested and violations
/// are present.
fn evaluate_drc(report: &Value, fail_on_violations: bool) -> Result<(), StageError> {
    let n = report
        .get("num_violations")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    if fail_on_violations && n > 0 {
        return Err(StageError::BackendError(format!(
            "DRC found {n} violation(s) and fail_on_violations is set"
        )));
    }
    Ok(())
}

#[async_trait]
impl Stage for DrcCheckStage {
    fn stage_type(&self) -> StageType {
        StageType::DrcCheck
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let mut chip_params = chip_params_for_drc(&input);

        // A foundry profile supplies deck + gating defaults; explicit params win.
        let foundry = input
            .get("foundry")
            .and_then(|v| v.as_str())
            .and_then(qservices_common::foundry::profile);

        // Deck precedence: explicit pdk/deck > foundry profile deck.
        let deck = input
            .get("pdk")
            .or_else(|| input.get("deck"))
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| foundry.map(|f| f.pdk_deck.to_string()));
        if let Some(deck) = &deck
            && let Some(obj) = chip_params.as_object_mut()
        {
            obj.insert("pdk".to_string(), json!(deck));
        }

        // Gating precedence: explicit fail_on_violations > profile gate_on_drc > false.
        let fail_on_violations = input
            .get("fail_on_violations")
            .and_then(|v| v.as_bool())
            .unwrap_or_else(|| foundry.map(|f| f.gate_on_drc).unwrap_or(false));

        let resp = self
            .client
            .post(format!("{}/drc", ctx.quantum_api_url))
            .json(&chip_params)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        let report = resp
            .json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))?;

        evaluate_drc(&report, fail_on_violations)?;
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn prefers_gds_generate_chip_params() {
        let input = json!({
            "gds_generate_output": { "chip_params": { "cols": 3, "rows": 2 } },
            "cols": 9
        });
        let params = chip_params_for_drc(&input);
        assert_eq!(params.get("cols"), Some(&json!(3)));
        assert_eq!(params.get("rows"), Some(&json!(2)));
    }

    #[test]
    fn falls_back_to_explicit_params() {
        let input = json!({ "cols": 4, "rows": 1 });
        let params = chip_params_for_drc(&input);
        assert_eq!(params.get("cols"), Some(&json!(4)));
        assert_eq!(params.get("rows"), Some(&json!(1)));
    }

    #[test]
    fn report_only_does_not_fail_on_violations() {
        let report = json!({ "clean": false, "num_violations": 7 });
        assert!(evaluate_drc(&report, false).is_ok());
    }

    #[test]
    fn gating_fails_on_violations() {
        let report = json!({ "clean": false, "num_violations": 7 });
        let result = evaluate_drc(&report, true);
        assert!(matches!(result, Err(StageError::BackendError(_))));
    }

    #[test]
    fn gating_passes_on_clean_layout() {
        let report = json!({ "clean": true, "num_violations": 0 });
        assert!(evaluate_drc(&report, true).is_ok());
    }
}
