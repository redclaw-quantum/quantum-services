use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::json;
use uuid::Uuid;

use crate::server::AppState;

pub async fn handle_artifacts(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let run = state.checkpoint.load(id).map_err(|_| StatusCode::NOT_FOUND)?;
    let artifacts: Vec<_> = run.artifact_keys.iter().map(|k| json!({ "key": k })).collect();
    Ok(Json(json!({ "pipeline_id": id, "artifacts": artifacts })))
}
