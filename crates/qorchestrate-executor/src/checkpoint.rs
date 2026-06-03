use std::path::{Path, PathBuf};

use anyhow::Result;
use uuid::Uuid;

use crate::state::PipelineRunState;

pub struct CheckpointStore {
    dir: PathBuf,
}

impl CheckpointStore {
    /// Create a new CheckpointStore backed by `dir`.
    /// Creates the directory if it doesn't exist.
    pub fn new(dir: impl AsRef<Path>) -> Result<Self> {
        let dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&dir)?;
        Ok(Self { dir })
    }

    /// Default location: /tmp/qorchestrate/checkpoints
    pub fn default_store() -> Result<Self> {
        Self::new("/tmp/qorchestrate/checkpoints")
    }

    /// Save the full PipelineRunState as JSON.
    pub fn save(&self, state: &PipelineRunState) -> Result<()> {
        let path = self.path_for(state.id);
        let json = serde_json::to_string_pretty(state)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load a PipelineRunState by run ID.
    pub fn load(&self, run_id: Uuid) -> Result<PipelineRunState> {
        let path = self.path_for(run_id);
        let json = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Checkpoint not found for {}: {}", run_id, e))?;
        Ok(serde_json::from_str(&json)?)
    }

    /// Check if a checkpoint exists.
    pub fn exists(&self, run_id: Uuid) -> bool {
        self.path_for(run_id).exists()
    }

    /// Delete a checkpoint.
    pub fn delete(&self, run_id: Uuid) -> Result<()> {
        let path = self.path_for(run_id);
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    fn path_for(&self, run_id: Uuid) -> PathBuf {
        self.dir.join(format!("{}.json", run_id))
    }

    /// Return the backing directory path.
    pub fn dir(&self) -> &std::path::Path {
        &self.dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_store() -> CheckpointStore {
        let dir = std::env::temp_dir().join(format!(
            "qorchestrate_test_{}",
            Uuid::new_v4()
        ));
        CheckpointStore::new(dir).expect("failed to create temp store")
    }

    #[test]
    fn test_save_and_load() {
        let store = temp_store();
        let run_id = Uuid::new_v4();
        let state =
            crate::state::PipelineRunState::new(run_id, "my-pipeline", "/brain/path", json!({"x": 1}));

        store.save(&state).expect("save failed");
        assert!(store.exists(run_id));

        let loaded = store.load(run_id).expect("load failed");
        assert_eq!(loaded.id, run_id);
        assert_eq!(loaded.template, "my-pipeline");
        assert_eq!(loaded.brain_path, "/brain/path");
        assert_eq!(loaded.params, json!({"x": 1}));
    }

    #[test]
    fn test_load_not_found() {
        let store = temp_store();
        let missing_id = Uuid::new_v4();
        let result = store.load(missing_id);
        assert!(result.is_err());
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("Checkpoint not found"));
    }
}
