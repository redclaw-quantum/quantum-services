use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Cross-platform QEC compiler stage: POSTs `{ platform, physical_error_rate?,
/// target_ler? }` to `/qec/compile`, which recommends a code + distance +
/// decoder (modality-aware) and the physical-qubit overhead.
pub struct QecCompileStage {
    client: Client,
}

impl QecCompileStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for QecCompileStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for QecCompileStage {
    fn stage_type(&self) -> StageType {
        StageType::QecCompile
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/qec/compile", ctx.quantum_api_url))
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
