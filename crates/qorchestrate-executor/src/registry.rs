use std::collections::HashMap;
use std::sync::Arc;

use qorchestrate_core::stage::{Stage, StageType};

pub type BoxedStage = Arc<dyn Stage + Send + Sync>;

pub struct StageRegistry {
    stages: HashMap<StageType, BoxedStage>,
}

impl StageRegistry {
    pub fn new() -> Self {
        Self {
            stages: HashMap::new(),
        }
    }

    pub fn register(&mut self, stage_type: StageType, stage: BoxedStage) {
        self.stages.insert(stage_type, stage);
    }

    pub fn get(&self, stage_type: &StageType) -> Option<&BoxedStage> {
        self.stages.get(stage_type)
    }

    pub fn has(&self, stage_type: &StageType) -> bool {
        self.stages.contains_key(stage_type)
    }
}

impl Default for StageRegistry {
    fn default() -> Self {
        Self::new()
    }
}
