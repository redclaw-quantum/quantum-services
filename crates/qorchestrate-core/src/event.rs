use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageEvent {
    pub pipeline_id: Uuid,
    pub stage_id: String,
    pub event_type: StageEventType,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: Option<u64>,
    pub input_summary: Option<Value>,
    pub output_summary: Option<Value>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StageEventType {
    Started,
    Progress { message: String },
    Completed,
    Failed,
    Skipped { reason: String },
    FallingBack { fallback_type: String },
    Retrying { attempt: usize },
    Timeout { timeout_secs: u64 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineEvent {
    pub pipeline_id: Uuid,
    pub event_type: PipelineEventType,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PipelineEventType {
    Started { template: String },
    Completed { duration_secs: u64, artifact_key: Option<String> },
    Failed { reason: String },
    Cancelled,
}

impl StageEvent {
    pub fn started(pipeline_id: Uuid, stage_id: impl Into<String>) -> Self {
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::Started,
            timestamp: Utc::now(),
            duration_ms: None,
            input_summary: None,
            output_summary: None,
            error: None,
        }
    }

    pub fn completed(
        pipeline_id: Uuid,
        stage_id: impl Into<String>,
        duration_ms: u64,
        output_summary: Option<Value>,
    ) -> Self {
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::Completed,
            timestamp: Utc::now(),
            duration_ms: Some(duration_ms),
            input_summary: None,
            output_summary,
            error: None,
        }
    }

    pub fn failed(
        pipeline_id: Uuid,
        stage_id: impl Into<String>,
        error: impl Into<String>,
    ) -> Self {
        let error_str = error.into();
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::Failed,
            timestamp: Utc::now(),
            duration_ms: None,
            input_summary: None,
            output_summary: None,
            error: Some(error_str),
        }
    }

    pub fn skipped(
        pipeline_id: Uuid,
        stage_id: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::Skipped { reason: reason.into() },
            timestamp: Utc::now(),
            duration_ms: None,
            input_summary: None,
            output_summary: None,
            error: None,
        }
    }

    pub fn falling_back(
        pipeline_id: Uuid,
        stage_id: impl Into<String>,
        fallback_type: impl Into<String>,
    ) -> Self {
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::FallingBack { fallback_type: fallback_type.into() },
            timestamp: Utc::now(),
            duration_ms: None,
            input_summary: None,
            output_summary: None,
            error: None,
        }
    }

    pub fn progress(
        pipeline_id: Uuid,
        stage_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            pipeline_id,
            stage_id: stage_id.into(),
            event_type: StageEventType::Progress { message: message.into() },
            timestamp: Utc::now(),
            duration_ms: None,
            input_summary: None,
            output_summary: None,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixed_id() -> Uuid {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    #[test]
    fn stage_event_started_fields() {
        let id = fixed_id();
        let ev = StageEvent::started(id, "freq_optimize");
        assert_eq!(ev.pipeline_id, id);
        assert_eq!(ev.stage_id, "freq_optimize");
        assert!(matches!(ev.event_type, StageEventType::Started));
        assert!(ev.duration_ms.is_none());
        assert!(ev.error.is_none());
    }

    #[test]
    fn stage_event_completed_has_duration() {
        let id = fixed_id();
        let ev = StageEvent::completed(id, "oqfp_build", 1234, None);
        assert_eq!(ev.stage_id, "oqfp_build");
        assert!(matches!(ev.event_type, StageEventType::Completed));
        assert_eq!(ev.duration_ms, Some(1234));
        assert!(ev.error.is_none());
    }

    #[test]
    fn stage_event_failed_has_error() {
        let id = fixed_id();
        let ev = StageEvent::failed(id, "scq_simulate", "timeout");
        assert!(matches!(ev.event_type, StageEventType::Failed));
        assert_eq!(ev.error.as_deref(), Some("timeout"));
        assert!(ev.duration_ms.is_none());
    }

    #[test]
    fn stage_event_skipped_has_reason() {
        let id = fixed_id();
        let ev = StageEvent::skipped(id, "pqec_assess", "condition not met");
        if let StageEventType::Skipped { reason } = &ev.event_type {
            assert_eq!(reason, "condition not met");
        } else {
            panic!("expected Skipped variant");
        }
    }

    #[test]
    fn stage_event_progress_has_message() {
        let id = fixed_id();
        let ev = StageEvent::progress(id, "grape_optimize", "iteration 50/200");
        if let StageEventType::Progress { message } = &ev.event_type {
            assert_eq!(message, "iteration 50/200");
        } else {
            panic!("expected Progress variant");
        }
    }

    #[test]
    fn stage_event_falling_back_has_type() {
        let id = fixed_id();
        let ev = StageEvent::falling_back(id, "freq_optimize", "skip");
        if let StageEventType::FallingBack { fallback_type } = &ev.event_type {
            assert_eq!(fallback_type, "skip");
        } else {
            panic!("expected FallingBack variant");
        }
    }

    #[test]
    fn stage_event_type_serde_started() {
        let et = StageEventType::Started;
        let json = serde_json::to_string(&et).unwrap();
        assert!(json.contains("started"));
        let et2: StageEventType = serde_json::from_str(&json).unwrap();
        assert!(matches!(et2, StageEventType::Started));
    }

    #[test]
    fn stage_event_type_serde_retrying() {
        let et = StageEventType::Retrying { attempt: 3 };
        let json = serde_json::to_string(&et).unwrap();
        assert!(json.contains("retrying"));
        let et2: StageEventType = serde_json::from_str(&json).unwrap();
        if let StageEventType::Retrying { attempt } = et2 {
            assert_eq!(attempt, 3);
        } else {
            panic!("expected Retrying");
        }
    }

    #[test]
    fn pipeline_event_type_started_serde() {
        let et = PipelineEventType::Started { template: "design-to-chip".to_string() };
        let json = serde_json::to_string(&et).unwrap();
        let et2: PipelineEventType = serde_json::from_str(&json).unwrap();
        if let PipelineEventType::Started { template } = et2 {
            assert_eq!(template, "design-to-chip");
        } else {
            panic!("expected Started");
        }
    }

    #[test]
    fn pipeline_event_type_completed_serde() {
        let et = PipelineEventType::Completed { duration_secs: 42, artifact_key: Some("result.json".to_string()) };
        let json = serde_json::to_string(&et).unwrap();
        let et2: PipelineEventType = serde_json::from_str(&json).unwrap();
        if let PipelineEventType::Completed { duration_secs, artifact_key } = et2 {
            assert_eq!(duration_secs, 42);
            assert_eq!(artifact_key.as_deref(), Some("result.json"));
        } else {
            panic!("expected Completed");
        }
    }

    #[test]
    fn pipeline_event_cancelled_serde() {
        let et = PipelineEventType::Cancelled;
        let json = serde_json::to_string(&et).unwrap();
        assert!(json.contains("cancelled"));
        let et2: PipelineEventType = serde_json::from_str(&json).unwrap();
        assert!(matches!(et2, PipelineEventType::Cancelled));
    }
}
