//! Persist the accumulated pipeline state to a JSON checkpoint — self-contained.
//!
//! Was a phantom: `StageType::Clawhdf5Save` existed with no impl. This writes the
//! stage input (the merged upstream outputs) to a JSON file so a run's state can
//! be archived / inspected / resumed. `save_dir` + `save_key` are read from the
//! pipeline params. (A future iteration can route into the clawhdf5 HDF5 store;
//! the contract — "persist this state under a key" — is the same.)

use async_trait::async_trait;
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct Clawhdf5SaveStage;

impl Clawhdf5SaveStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for Clawhdf5SaveStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Persist the pipeline state to `{save_dir}/{save_key}.json` (pure; no ctx).
pub fn save_state(input: &Value) -> Result<Value, StageError> {
    let dir = input.get("save_dir").and_then(|v| v.as_str()).unwrap_or("/tmp");
    let key = input.get("save_key").and_then(|v| v.as_str()).unwrap_or("pipeline_state");
    let path = format!("{dir}/{key}.json");

    let bytes = serde_json::to_vec_pretty(input).map_err(|e| StageError::ParseError(e.to_string()))?;
    let n_bytes = bytes.len();
    std::fs::write(&path, &bytes)
        .map_err(|e| StageError::BackendError(format!("clawhdf5_save: write {path}: {e}")))?;

    Ok(json!({ "saved": true, "path": path, "key": key, "n_bytes": n_bytes }))
}

#[async_trait]
impl Stage for Clawhdf5SaveStage {
    fn stage_type(&self) -> StageType {
        StageType::Clawhdf5Save
    }

    fn timeout_secs(&self) -> u64 {
        10
    }

    async fn execute_raw(&self, input: Value, _ctx: &StageContext) -> Result<Value, StageError> {
        save_state(&input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persists_state_to_json() {
        let dir = std::env::temp_dir();
        let input = json!({ "save_dir": dir.to_str().unwrap(), "save_key": "fmea_test_ckpt", "payload": { "a": 1 } });
        let out = save_state(&input).unwrap();
        assert_eq!(out.get("saved").unwrap(), &json!(true));
        let path = out.get("path").unwrap().as_str().unwrap();
        let read = std::fs::read_to_string(path).unwrap();
        assert!(read.contains("payload"));
        let _ = std::fs::remove_file(path);
    }
}
