use thiserror::Error;

#[derive(Debug, Error)]
pub enum PipelineError {
    #[error("Pipeline definition invalid: {0}")]
    InvalidDefinition(String),

    #[error("Stage '{stage}' failed: {reason}")]
    StageFailed { stage: String, reason: String },

    #[error("Stage '{stage}' timed out after {timeout_secs}s")]
    StageTimeout { stage: String, timeout_secs: u64 },

    #[error("Pipeline '{id}' not found")]
    NotFound { id: String },

    #[error("DAG cycle detected involving stage '{stage}'")]
    CycleDetected { stage: String },

    #[error("Checkpoint error: {0}")]
    CheckpointError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("Max nesting depth ({max}) exceeded")]
    NestingDepthExceeded { max: usize },

    #[error("Stage '{stage}' condition field '{field}' not found in output")]
    ConditionFieldNotFound { stage: String, field: String },
}

#[derive(Debug, Error)]
pub enum StageError {
    #[error("HTTP request failed: {0}")]
    HttpError(String),

    #[error("Response parse error: {0}")]
    ParseError(String),

    #[error("Stage timeout")]
    Timeout,

    #[error("Stage skipped by condition")]
    SkippedByCondition,

    #[error("Invalid input: {0}")]
    InvalidInput(String),

    #[error("Backend error: {0}")]
    BackendError(String),

    #[error("Serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipeline_error_invalid_definition_display() {
        let e = PipelineError::InvalidDefinition("missing stage".to_string());
        let msg = e.to_string();
        assert!(msg.contains("invalid"));
        assert!(msg.contains("missing stage"));
    }

    #[test]
    fn pipeline_error_stage_failed_display() {
        let e = PipelineError::StageFailed {
            stage: "freq_optimize".to_string(),
            reason: "HTTP 500".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("freq_optimize"));
        assert!(msg.contains("HTTP 500"));
    }

    #[test]
    fn pipeline_error_stage_timeout_display() {
        let e = PipelineError::StageTimeout { stage: "grape_optimize".to_string(), timeout_secs: 300 };
        let msg = e.to_string();
        assert!(msg.contains("grape_optimize"));
        assert!(msg.contains("300"));
    }

    #[test]
    fn pipeline_error_not_found_display() {
        let e = PipelineError::NotFound { id: "abc-123".to_string() };
        let msg = e.to_string();
        assert!(msg.contains("abc-123"));
        assert!(msg.contains("not found"));
    }

    #[test]
    fn pipeline_error_cycle_detected_display() {
        let e = PipelineError::CycleDetected { stage: "oqfp_build".to_string() };
        let msg = e.to_string();
        assert!(msg.contains("oqfp_build"));
        assert!(msg.contains("cycle"));
    }

    #[test]
    fn pipeline_error_nesting_depth_exceeded_display() {
        let e = PipelineError::NestingDepthExceeded { max: 5 };
        let msg = e.to_string();
        assert!(msg.contains("5"));
        assert!(msg.contains("depth") || msg.contains("nesting"));
    }

    #[test]
    fn pipeline_error_condition_field_not_found_display() {
        let e = PipelineError::ConditionFieldNotFound {
            stage: "pqec_assess".to_string(),
            field: "threshold_met".to_string(),
        };
        let msg = e.to_string();
        assert!(msg.contains("pqec_assess"));
        assert!(msg.contains("threshold_met"));
    }

    #[test]
    fn stage_error_http_error_display() {
        let e = StageError::HttpError("connection refused".to_string());
        let msg = e.to_string();
        assert!(msg.contains("HTTP") || msg.contains("http") || msg.contains("connection refused"));
    }

    #[test]
    fn stage_error_parse_error_display() {
        let e = StageError::ParseError("invalid JSON field".to_string());
        let msg = e.to_string();
        assert!(msg.contains("invalid JSON field"));
    }

    #[test]
    fn stage_error_timeout_display() {
        let e = StageError::Timeout;
        let msg = e.to_string();
        assert!(msg.contains("timeout") || msg.contains("Timeout"));
    }

    #[test]
    fn stage_error_skipped_by_condition_display() {
        let e = StageError::SkippedByCondition;
        let msg = e.to_string();
        assert!(msg.contains("condition") || msg.contains("skipped"));
    }

    #[test]
    fn stage_error_invalid_input_display() {
        let e = StageError::InvalidInput("qubit count must be > 0".to_string());
        let msg = e.to_string();
        assert!(msg.contains("qubit count must be > 0"));
    }

    #[test]
    fn stage_error_backend_error_display() {
        let e = StageError::BackendError("qem-rs unavailable".to_string());
        let msg = e.to_string();
        assert!(msg.contains("qem-rs unavailable"));
    }
}
