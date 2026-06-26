//! Closes the post-fab loop: consume the digital twin's `TriggerInverseDesign`
//! recalibration hints and re-invoke QPUDIDP inverse design *seeded by each hint*.
//!
//! Before this stage, `full-loop.toml` reacted to a failed chip by blanket
//! re-running the whole `design-to-chip` pipeline, ignoring the structured
//! `InverseDesignHint` (target frequency / anharmonicity / warm-start geometry)
//! the twin had already computed. This stage performs the *targeted* re-design
//! the hint was designed for — the loop is now genuinely closed.

use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct RecalDispatchStage {
    client: Client,
}

impl RecalDispatchStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for RecalDispatchStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Recursively collect every `InverseDesignHint` carried by a
/// `TriggerInverseDesign` recalibration action anywhere in the recal output.
/// Robust to the exact nesting (`recal_actions.suggestions[].action.…`).
pub fn collect_redesign_hints(v: &Value) -> Vec<Value> {
    let mut out = Vec::new();
    walk(v, &mut out);
    out
}

fn walk(v: &Value, out: &mut Vec<Value>) {
    match v {
        Value::Object(map) => {
            // RecalAction is externally tagged → {"TriggerInverseDesign": {"hint": …}}
            if let Some(hint) = map.get("TriggerInverseDesign").and_then(|t| t.get("hint")) {
                out.push(hint.clone());
            }
            for child in map.values() {
                walk(child, out);
            }
        }
        Value::Array(arr) => {
            for child in arr {
                walk(child, out);
            }
        }
        _ => {}
    }
}

/// Map an `InverseDesignHint` to a `/pipeline/design` request body.
pub fn hint_to_design_req(hint: &Value) -> Value {
    json!({
        "device_type": hint.get("device_type").cloned().unwrap_or_else(|| json!("TransmonCross")),
        "qubit_frequency_ghz": hint.get("qubit_frequency_ghz").cloned().unwrap_or_else(|| json!(5.0)),
        "anharmonicity_mhz": hint.get("anharmonicity_mhz").cloned().unwrap_or_else(|| json!(-200.0)),
        "max_candidates": 3,
    })
}

#[async_trait]
impl Stage for RecalDispatchStage {
    fn stage_type(&self) -> StageType {
        StageType::RecalDispatch
    }

    fn timeout_secs(&self) -> u64 {
        120
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let hints = collect_redesign_hints(&input);
        if hints.is_empty() {
            return Ok(json!({
                "redesign_triggered": false,
                "num_hints": 0,
                "redesigns": [],
            }));
        }

        let mut redesigns = Vec::new();
        for hint in &hints {
            let body = hint_to_design_req(hint);
            let resp = self
                .client
                .post(format!("{}/pipeline/design", ctx.quantum_api_url))
                .json(&body)
                .send()
                .await
                .map_err(|e| StageError::HttpError(e.to_string()))?;
            if !resp.status().is_success() {
                let status = resp.status();
                let b = resp.text().await.unwrap_or_default();
                return Err(StageError::BackendError(format!("{}: {}", status, b)));
            }
            let result: Value = resp
                .json()
                .await
                .map_err(|e| StageError::ParseError(e.to_string()))?;
            redesigns.push(json!({
                "qubit_id": hint.get("qubit_id").cloned().unwrap_or(Value::Null),
                "reason": hint.get("reason").cloned().unwrap_or(Value::Null),
                "target_frequency_ghz": hint.get("qubit_frequency_ghz").cloned().unwrap_or(Value::Null),
                "redesign": result,
            }));
        }

        Ok(json!({
            "redesign_triggered": true,
            "num_hints": hints.len(),
            "redesigns": redesigns,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recal_output() -> Value {
        // Shape mirrors `qtwin recalibrate --json`: recal_actions wrapping a list
        // of suggestions, each with an externally-tagged RecalAction.
        json!({
            "recal_actions": {
                "critical_count": 1,
                "suggestions": [
                    { "qubit_id": 0, "action": { "AdjustFluxBias": { "target_flux": 0.1 } } },
                    { "qubit_id": 3, "action": { "TriggerInverseDesign": { "hint": {
                        "qubit_id": 3, "device_type": "TransmonCross",
                        "qubit_frequency_ghz": 5.12, "anharmonicity_mhz": -198.0,
                        "original_geometry": [0.3, 0.02, 0.01, 0.1, 0.01, 0.005, 12.0],
                        "reason": "T1 collapsed to 8% of design"
                    }}}}
                ]
            }
        })
    }

    #[test]
    fn collects_only_trigger_inverse_design_hints() {
        let hints = collect_redesign_hints(&recal_output());
        assert_eq!(hints.len(), 1);
        assert_eq!(hints[0].get("qubit_id").unwrap(), &json!(3));
        assert_eq!(hints[0].get("qubit_frequency_ghz").unwrap(), &json!(5.12));
    }

    #[test]
    fn maps_hint_to_targeted_design_request() {
        let hints = collect_redesign_hints(&recal_output());
        let req = hint_to_design_req(&hints[0]);
        // The re-design is seeded by the hint's targets — not a blanket default.
        assert_eq!(req.get("qubit_frequency_ghz").unwrap(), &json!(5.12));
        assert_eq!(req.get("anharmonicity_mhz").unwrap(), &json!(-198.0));
        assert_eq!(req.get("device_type").unwrap(), &json!("TransmonCross"));
    }

    #[test]
    fn no_hints_when_no_redesign_needed() {
        let healthy = json!({ "recal_actions": { "critical_count": 0, "suggestions": [
            { "qubit_id": 0, "action": { "RetunePulse": { "new_amplitude": 0.5, "new_duration_ns": 30.0 } } }
        ]}});
        assert!(collect_redesign_hints(&healthy).is_empty());
    }
}
