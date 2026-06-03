use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// POST /qcirc/processes — identify all parametric processes in a QuantizedCircuit.
///
/// Input: `QuantizedCircuit` (or wrapped as `{ "circuit": { ... } }`).
/// Output: list of `ProcessInfo` with type (3WM/4WM/Kerr/cross-Kerr), coupling
/// strength, and suggested pump frequency for each active process.
pub struct QcircProcessesStage {
    client: Client,
}

impl QcircProcessesStage {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl Default for QcircProcessesStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QcircProcessesStage {
    fn stage_type(&self) -> StageType {
        StageType::QcircProcesses
    }

    fn timeout_secs(&self) -> u64 {
        15
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        // Accept either direct QuantizedCircuit or wrapped from qcirc_quantize output
        let body = if input.get("modes").is_some() {
            input
        } else if let Some(qc) = input
            .get("qcirc_quantize_output")
            .and_then(|v| v.as_object())
            .and_then(|o| o.get("circuit"))
        {
            qc.clone()
        } else {
            input
        };

        let resp = self
            .client
            .post(format!("{}/qcirc/processes", ctx.quantum_api_url))
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
