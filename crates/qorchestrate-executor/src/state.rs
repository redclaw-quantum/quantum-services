use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PipelineStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
    Paused,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StageRunStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
    FallingBack,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageRunState {
    pub stage_id: String,
    pub status: StageRunStatus,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub error: Option<String>,
    pub output: Option<Value>,
    pub skipped_reason: Option<String>,
    pub used_fallback: bool,
    pub attempts: usize,
}

impl StageRunState {
    pub fn new(stage_id: impl Into<String>) -> Self {
        Self {
            stage_id: stage_id.into(),
            status: StageRunStatus::Pending,
            started_at: None,
            completed_at: None,
            duration_ms: None,
            error: None,
            output: None,
            skipped_reason: None,
            used_fallback: false,
            attempts: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineRunState {
    pub id: Uuid,
    pub template: String,
    pub status: PipelineStatus,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub stages: HashMap<String, StageRunState>,
    pub output: Option<Value>,
    pub artifact_keys: Vec<String>,
    pub brain_path: String,
    pub params: Value,
}

impl PipelineRunState {
    pub fn new(
        id: Uuid,
        template: impl Into<String>,
        brain_path: impl Into<String>,
        params: Value,
    ) -> Self {
        Self {
            id,
            template: template.into(),
            status: PipelineStatus::Queued,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            stages: HashMap::new(),
            output: None,
            artifact_keys: Vec::new(),
            brain_path: brain_path.into(),
            params,
        }
    }

    pub fn elapsed_secs(&self) -> u64 {
        let start = self.started_at.unwrap_or(self.created_at);
        (Utc::now() - start).num_seconds().max(0) as u64
    }
}
