use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct ScqSimulateStage {
    client: Client,
}

impl ScqSimulateStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for ScqSimulateStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for ScqSimulateStage {
    fn stage_type(&self) -> StageType {
        StageType::ScqSimulate
    }

    fn timeout_secs(&self) -> u64 {
        10
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/scq/simulate", ctx.quantum_api_url))
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
