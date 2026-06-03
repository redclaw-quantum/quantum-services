use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// POST /qcirc/regime-scan — sweep pump amplitude × frequency and find
/// collision-free operational regimes via Pareto optimization.
///
/// Input: `{ "circuit": QuantizedCircuit, "pump_type": str,
///            "amp_range": [min, max], "freq_range": [min, max],
///            "n_amp": int, "n_freq": int }`.
/// Output: `RegimeScanResult` with Pareto-optimal `RegimePoint`s
/// (coupling, heating rate, min gap, circuit constraints).
pub struct QcircRegimeScanStage {
    client: Client,
}

impl QcircRegimeScanStage {
    pub fn new() -> Self {
        Self { client: Client::new() }
    }
}

impl Default for QcircRegimeScanStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QcircRegimeScanStage {
    fn stage_type(&self) -> StageType {
        StageType::QcircRegimeScan
    }

    fn timeout_secs(&self) -> u64 {
        120
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qcirc/regime-scan", ctx.quantum_api_url))
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
