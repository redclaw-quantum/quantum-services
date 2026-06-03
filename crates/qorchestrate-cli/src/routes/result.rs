use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::json;
use uuid::Uuid;

use qorchestrate_executor::PipelineStatus;

use crate::server::AppState;

pub async fn handle_result(
    Path(id): Path<Uuid>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let run = {
        let guard = state.runs.read().await;
        guard.get(&id).cloned()
    };
    let run = match run {
        Some(r) => r,
        None => state.checkpoint.load(id).map_err(|_| StatusCode::NOT_FOUND)?,
    };

    if run.status == PipelineStatus::Running || run.status == PipelineStatus::Queued {
        return Err(StatusCode::ACCEPTED);
    }

    let duration_secs = run.elapsed_secs();
    Ok(Json(json!({
        "pipeline_id": run.id,
        "status": run.status,
        "template": run.template,
        "duration_secs": duration_secs,
        "output": run.output,
        "artifact_keys": run.artifact_keys,
    })))
}
