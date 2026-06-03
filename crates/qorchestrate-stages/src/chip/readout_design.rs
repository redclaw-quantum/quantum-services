use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct ReadoutDesignStage {
    client: Client,
}

impl ReadoutDesignStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for ReadoutDesignStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for ReadoutDesignStage {
    fn stage_type(&self) -> StageType {
        StageType::ReadoutDesign
    }

    fn timeout_secs(&self) -> u64 {
        15
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/readout/design", ctx.quantum_api_url))
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
