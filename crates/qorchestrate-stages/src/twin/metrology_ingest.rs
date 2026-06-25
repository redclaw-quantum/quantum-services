use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Ingest a raw cryo-measurement record (the post-fab metrology loop): POSTs the
/// stage input (`{ measurement, design, recalibrate? }`) to `/qtwin/ingest`,
/// which compares the measurement against the design and returns the digital
/// twin + recalibration suggestions. The `design` can be an upstream OQFP spec
/// or DesignSpec; `measurement` is a `CryoMeasurementRecord`.
pub struct MetrologyIngestStage {
    client: Client,
}

impl MetrologyIngestStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for MetrologyIngestStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for MetrologyIngestStage {
    fn stage_type(&self) -> StageType {
        StageType::MetrologyIngest
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qtwin/ingest", ctx.quantum_api_url))
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
