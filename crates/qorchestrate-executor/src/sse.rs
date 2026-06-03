use qorchestrate_core::event::{StageEvent, StageEventType};
use tokio::sync::broadcast;

pub struct SseBroadcaster {
    tx: broadcast::Sender<StageEvent>,
}

impl SseBroadcaster {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    pub fn sender(&self) -> broadcast::Sender<StageEvent> {
        self.tx.clone()
    }

    pub fn subscribe(&self) -> broadcast::Receiver<StageEvent> {
        self.tx.subscribe()
    }

    /// Convert a StageEvent to its SSE event name (used in `event: <name>` line).
    pub fn event_name(event: &StageEvent) -> &'static str {
        match &event.event_type {
            StageEventType::Started => "stage_started",
            StageEventType::Progress { .. } => "stage_progress",
            StageEventType::Completed => "stage_completed",
            StageEventType::Failed => "stage_failed",
            StageEventType::Skipped { .. } => "stage_skipped",
            StageEventType::FallingBack { .. } => "stage_fallback",
            StageEventType::Retrying { .. } => "stage_retrying",
            StageEventType::Timeout { .. } => "stage_timeout",
        }
    }

    /// Format a StageEvent as an SSE data line: `event: <name>\ndata: <json>\n\n`
    pub fn format_sse(event: &StageEvent) -> String {
        let name = Self::event_name(event);
        let data = serde_json::to_string(event).unwrap_or_default();
        format!("event: {name}\ndata: {data}\n\n")
    }
}
