use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use chrono::Utc;
use serde_json::json;
use uuid::Uuid;

use qorchestrate_executor::PipelineStatus;

use crate::server::AppState;

pub async fn handle_cancel(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let mut runs = state.runs.write().await;
    if let Some(run) = runs.get_mut(&id) {
        run.status = PipelineStatus::Cancelled;
        run.completed_at = Some(Utc::now());
        Ok(Json(json!({ "cancelled": true, "pipeline_id": id })))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}
