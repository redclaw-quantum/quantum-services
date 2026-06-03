use async_trait::async_trait;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Returns its input verbatim. The orchestrator's `build_stage_input`
/// merges pipeline params, this stage's `params` block, and
/// `<dep_id>_output` keys for every dependency — so consuming the input as
/// the output gives you a structured snapshot of everything this stage's
/// upstream pipeline branches produced. Use as a terminal stage when a
/// template needs to expose multiple upstream outputs in one place.
pub struct CollectDepsStage;

impl CollectDepsStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for CollectDepsStage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Stage for CollectDepsStage {
    fn stage_type(&self) -> StageType {
        StageType::CollectDeps
    }

    fn timeout_secs(&self) -> u64 {
        1
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        Ok(input)
    }
}
