use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use uuid::Uuid;

use crate::server::AppState;

pub async fn handle_status(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check in-memory first.
    if let Some(run) = state.runs.read().await.get(&id) {
        return Ok(Json(serde_json::to_value(run).unwrap_or_default()));
    }
    // Fall back to checkpoint.
    match state.checkpoint.load(id) {
        Ok(run) => Ok(Json(serde_json::to_value(&run).unwrap_or_default())),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}
