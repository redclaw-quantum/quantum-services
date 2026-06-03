use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct SkipStage;

impl SkipStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for SkipStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for SkipStage {
    fn stage_type(&self) -> StageType {
        StageType::Skip
    }

    fn timeout_secs(&self) -> u64 {
        1
    }

    async fn execute_raw(&self, _input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        Ok(json!({ "skipped": true }))
    }
}
