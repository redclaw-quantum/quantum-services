use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// POST /qcirc/constraints — extract circuit parameter constraints from a
/// regime scan result.
///
/// Input: `RegimeScanResult` (output of qcirc_regime_scan).
/// Output: `CircuitConstraints` with allowed ranges for each circuit parameter
/// (EJ, EC, α, flux bias, junction area) that support the target process in
/// a collision-free regime.
pub struct QcircConstraintsStage {
    client: Client,
}

impl QcircConstraintsStage {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl Default for QcircConstraintsStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QcircConstraintsStage {
    fn stage_type(&self) -> StageType {
        StageType::QcircConstraints
    }

    fn timeout_secs(&self) -> u64 {
        15
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qcirc/constraints", ctx.quantum_api_url))
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
