use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Metrology *acquisition* stage — the software-closed post-fab loop. POSTs the
/// stage input (`{ design, seed?, freq_offsets?, freq_sigma_mhz?, ... }`) to
/// `/qtwin/characterize`, which runs the simulated-fridge characterization
/// (spectroscopy/T1/T2/readout fits → `CryoMeasurementRecord`), compares it
/// against the design, and returns the digital twin + recalibration. Unlike
/// `MetrologyIngest`, no hand-supplied measurement record is required — the
/// instrument backend produces it (a real driver swaps in later).
pub struct MetrologyAcquireStage {
    client: Client,
}

impl MetrologyAcquireStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for MetrologyAcquireStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for MetrologyAcquireStage {
    fn stage_type(&self) -> StageType {
        StageType::MetrologyAcquire
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qtwin/characterize", ctx.quantum_api_url))
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
