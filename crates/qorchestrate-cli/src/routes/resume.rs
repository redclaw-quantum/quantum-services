use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::json;
use uuid::Uuid;

use qorchestrate_core::pipeline::PipelineDef;
use qorchestrate_executor::StageRunStatus;

use crate::server::AppState;

#[derive(Deserialize)]
pub struct ResumeRequest {
    pub pipeline_id: Uuid,
}

pub async fn handle_resume(
    State(state): State<AppState>,
    Json(req): Json<ResumeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let id = req.pipeline_id;
    let saved = state
        .checkpoint
        .load(id)
        .map_err(|_| (StatusCode::NOT_FOUND, format!("No checkpoint for {id}")))?;

    let toml_path = state
        .templates_dir
        .join(format!("{}.toml", saved.template));
    let toml_str = std::fs::read_to_string(&toml_path).map_err(|_| {
        (
            StatusCode::NOT_FOUND,
            format!("Template '{}' not found", saved.template),
        )
    })?;
    let def =
        PipelineDef::from_toml(&toml_str).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Find first failed/pending stage to report.
    let resume_from = saved
        .stages
        .iter()
        .filter(|(_, s)| {
            matches!(
                s.status,
                StageRunStatus::Failed | StageRunStatus::Pending
            )
        })
        .map(|(id, _)| id.clone())
        .next()
        .unwrap_or_default();

    // Spawn background resume.
    let executor = state.executor.clone();
    let runs = state.runs.clone();
    tokio::spawn(async move {
        match executor.resume_pipeline(id, &def).await {
            Ok(final_state) => {
                runs.write().await.insert(id, final_state);
            }
            Err(e) => {
                tracing::error!("Resume {id} failed: {e}");
            }
        }
    });

    Ok(Json(json!({
        "pipeline_id": id,
        "resumed_from_stage": resume_from,
        "status": "running"
    })))
}
