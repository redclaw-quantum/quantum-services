use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct CmaesStage {
    client: Client,
}

impl CmaesStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for CmaesStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for CmaesStage {
    fn stage_type(&self) -> StageType {
        StageType::QpudidpCmaes
    }

    fn timeout_secs(&self) -> u64 {
        60
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}/tools/inverse_design", ctx.qpudidp_url))
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
