use std::collections::HashMap;

use serde_json::Value;
use uuid::Uuid;

/// Cache key = (pipeline_run_id, stage_id)
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CacheKey {
    pub run_id: Uuid,
    pub stage_id: String,
}

pub struct StageResultCache {
    entries: HashMap<CacheKey, Value>,
}

impl StageResultCache {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn put(&mut self, run_id: Uuid, stage_id: impl Into<String>, value: Value) {
        self.entries.insert(
            CacheKey {
                run_id,
                stage_id: stage_id.into(),
            },
            value,
        );
    }

    pub fn get(&self, run_id: Uuid, stage_id: &str) -> Option<&Value> {
        self.entries.get(&CacheKey {
            run_id,
            stage_id: stage_id.to_string(),
        })
    }

    pub fn invalidate(&mut self, run_id: Uuid, stage_id: &str) {
        self.entries.remove(&CacheKey {
            run_id,
            stage_id: stage_id.to_string(),
        });
    }

    pub fn clear_run(&mut self, run_id: Uuid) {
        self.entries.retain(|k, _| k.run_id != run_id);
    }
}

impl Default for StageResultCache {
    fn default() -> Self {
        Self::new()
    }
}
