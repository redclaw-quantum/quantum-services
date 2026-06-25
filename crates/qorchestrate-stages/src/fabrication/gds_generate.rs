use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Generate a GDS-II chip layout from upstream design output.
///
/// Calls the quantum-api `/gds/export-chip` endpoint with a chip-layout request
/// derived from the frequency plan (qubit count → grid). Explicit stage params
/// (`cols`, `rows`, `pitch_x`, `pitch_y`, `qubit_params`) override the derived
/// values. The chip-layout request that was actually used is echoed back under
/// `chip_params` so the downstream `drc_check` stage can rebuild the identical
/// layout deterministically.
pub struct GdsGenerateStage {
    client: Client,
}

impl GdsGenerateStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for GdsGenerateStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Build the `/gds/export-chip` request body from this stage's input.
///
/// Precedence: explicit `cols`/`rows` stage params win; otherwise the grid is
/// derived from the number of frequency-plan assignments (falling back to 4
/// qubits when no upstream plan is present). `pitch_*` and `qubit_params` are
/// forwarded verbatim when supplied.
fn chip_params_from_input(input: &Value) -> Value {
    let explicit_cols = input.get("cols").and_then(|v| v.as_u64());
    let explicit_rows = input.get("rows").and_then(|v| v.as_u64());

    let n_qubits = input
        .get("freq_plan_output")
        .and_then(|f| f.get("assignments"))
        .and_then(|a| a.as_array())
        .map(|a| a.len())
        .filter(|n| *n > 0)
        .unwrap_or(4);

    let (cols, rows) = match (explicit_cols, explicit_rows) {
        (Some(c), Some(r)) => (c.max(1) as usize, r.max(1) as usize),
        _ => {
            let cols = (n_qubits as f64).sqrt().ceil() as usize;
            let cols = cols.max(1);
            let rows = n_qubits.div_ceil(cols);
            (cols, rows.max(1))
        }
    };

    let mut params = json!({ "cols": cols, "rows": rows });
    let obj = params.as_object_mut().expect("params is an object");
    for key in ["pitch_x", "pitch_y", "qubit_params"] {
        if let Some(v) = input.get(key) {
            obj.insert(key.to_string(), v.clone());
        }
    }

    // Tape-out layer map: explicit `layer_map` wins, else the foundry profile's.
    let layer_map = input
        .get("layer_map")
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .or_else(|| {
            input
                .get("foundry")
                .and_then(|v| v.as_str())
                .and_then(qservices_common::foundry::profile)
                .map(|p| p.layer_map.to_string())
        });
    if let Some(lm) = layer_map {
        obj.insert("layer_map".to_string(), json!(lm));
    }

    // Tape-out fab-prep frame (alignment marks + dicing lanes): on by default
    // for the pipeline so the bundled chip.gds is submission-ready.
    let frame = input
        .get("tapeout_frame")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    obj.insert("tapeout_frame".to_string(), json!(frame));

    // Dummy fill (CMP / etch-loading uniformity): on by default for the pipeline.
    let fill = input
        .get("dummy_fill")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    obj.insert("dummy_fill".to_string(), json!(fill));
    params
}

#[async_trait]
impl Stage for GdsGenerateStage {
    fn stage_type(&self) -> StageType {
        StageType::GdsGenerate
    }

    fn timeout_secs(&self) -> u64 {
        60
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let chip_params = chip_params_from_input(&input);

        let resp = self
            .client
            .post(format!("{}/gds/export-chip", ctx.quantum_api_url))
            .json(&chip_params)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        let mut out = resp
            .json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))?;

        // Echo the exact layout request so drc_check rebuilds the same layout.
        if let Some(obj) = out.as_object_mut() {
            obj.insert("chip_params".to_string(), chip_params);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn derives_grid_from_freq_plan_assignments() {
        let input = json!({
            "freq_plan_output": {
                "assignments": [
                    {"qubit": 0}, {"qubit": 1}, {"qubit": 2},
                    {"qubit": 3}, {"qubit": 4}
                ]
            }
        });
        let params = chip_params_from_input(&input);
        // 5 qubits -> cols = ceil(sqrt(5)) = 3, rows = ceil(5/3) = 2
        assert_eq!(params.get("cols"), Some(&json!(3)));
        assert_eq!(params.get("rows"), Some(&json!(2)));
    }

    #[test]
    fn explicit_params_override_derived_grid() {
        let input = json!({
            "cols": 4,
            "rows": 1,
            "pitch_x": 600.0,
            "freq_plan_output": { "assignments": [{"qubit": 0}, {"qubit": 1}] }
        });
        let params = chip_params_from_input(&input);
        assert_eq!(params.get("cols"), Some(&json!(4)));
        assert_eq!(params.get("rows"), Some(&json!(1)));
        assert_eq!(params.get("pitch_x"), Some(&json!(600.0)));
    }

    #[test]
    fn falls_back_to_default_when_no_upstream() {
        let params = chip_params_from_input(&json!({}));
        // 4 qubits default -> cols = 2, rows = 2
        assert_eq!(params.get("cols"), Some(&json!(2)));
        assert_eq!(params.get("rows"), Some(&json!(2)));
    }
}
