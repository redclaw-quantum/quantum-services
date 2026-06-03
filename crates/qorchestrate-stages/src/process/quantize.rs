use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// POST /qcirc/quantize — node-flux quantization of a circuit netlist.
///
/// Input: `{ "netlist": { ... } }` or a raw `NetlistSpec` JSON.
/// Output: `QuantizedCircuit` with modes, frequencies, ZPFs, nonlinear terms.
pub struct QcircQuantizeStage {
    client: Client,
}

impl QcircQuantizeStage {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl Default for QcircQuantizeStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QcircQuantizeStage {
    fn stage_type(&self) -> StageType {
        StageType::QcircQuantize
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qcirc/quantize", ctx.quantum_api_url))
            .json(&input)
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
