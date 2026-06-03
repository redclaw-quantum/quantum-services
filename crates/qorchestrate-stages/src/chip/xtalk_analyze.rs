use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct XtalkAnalyzeStage {
    client: Client,
}

impl XtalkAnalyzeStage {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl Default for XtalkAnalyzeStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for XtalkAnalyzeStage {
    fn stage_type(&self) -> StageType {
        StageType::XtalkAnalyze
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        // First call: ZZ crosstalk
        let zz_resp = self
            .client
            .post(format!("{}/xtalk/zz", ctx.quantum_api_url))
            .json(&input)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !zz_resp.status().is_success() {
            let status = zz_resp.status();
            let body = zz_resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        let zz: Value = zz_resp
            .json()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))?;

        // Second call: coupling matrix
        let coupling_resp = self
            .client
            .post(format!("{}/xtalk/coupling", ctx.quantum_api_url))
            .json(&input)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;

        if !coupling_resp.status().is_success() {
            let status = coupling_resp.status();
            let body = coupling_resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, body)));
        }

        let coupling: Value = coupling_resp
            .json()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))?;

        Ok(json!({ "zz": zz, "coupling": coupling }))
    }
}
