use axum::{extract::State, http::StatusCode, Json};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use qorchestrate_core::pipeline::PipelineDef;
use qorchestrate_executor::{PipelineRunState, PipelineStatus};

use crate::server::AppState;

#[derive(Deserialize)]
pub struct RunRequest {
    pub template: String,
    #[serde(default)]
    pub params: Value,
    pub brain_path: Option<String>,
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize)]
pub struct RunResponse {
    pub pipeline_id: Uuid,
    pub status: String,
    pub template: String,
    pub stage_count: usize,
    pub created_at: String,
}

pub async fn handle_run(
    State(state): State<AppState>,
    Json(req): Json<RunRequest>,
) -> Result<(StatusCode, Json<RunResponse>), (StatusCode, String)> {
    let toml_path = state.templates_dir.join(format!("{}.toml", req.template));
    let toml_str = std::fs::read_to_string(&toml_path)
        .map_err(|_| (StatusCode::NOT_FOUND, format!("Template '{}' not found", req.template)))?;
    let def = PipelineDef::from_toml(&toml_str)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Template parse error: {e}")))?;

    if req.dry_run {
        def.validate()
            .map_err(|errs| (StatusCode::BAD_REQUEST, errs.join("; ")))?;
        let run_id = Uuid::now_v7();
        return Ok((
            StatusCode::OK,
            Json(RunResponse {
                pipeline_id: run_id,
                status: "dry_run_ok".to_string(),
                template: req.template,
                stage_count: def.stages.len(),
                created_at: Utc::now().to_rfc3339(),
            }),
        ));
    }

    let run_id = Uuid::now_v7();
    let stage_count = def.stages.len();
    let brain_path = req.brain_path.unwrap_or_else(|| state.brain_path.clone());
    let params = req.params;
    let template_name = req.template.clone();

    // Create initial run state and insert into shared map.
    let initial_state = PipelineRunState::new(run_id, &template_name, &brain_path, params.clone());
    state.runs.write().await.insert(run_id, initial_state);

    // Create SSE broadcast channel for this run.
    let (event_tx, _) = tokio::sync::broadcast::channel(256);
    state.event_channels.write().await.insert(run_id, event_tx);

    // Spawn background execution.
    let executor = state.executor.clone();
    let runs = state.runs.clone();
    let def_clone = def.clone();
    let brain_path_clone = brain_path.clone();
    tokio::spawn(async move {
        match executor
            .run_pipeline(&def_clone, params, &brain_path_clone)
            .await
        {
            Ok(final_state) => {
                runs.write().await.insert(run_id, final_state);
            }
            Err(e) => {
                tracing::error!("Pipeline {run_id} failed: {e}");
                if let Some(s) = runs.write().await.get_mut(&run_id) {
                    s.status = PipelineStatus::Failed;
                }
            }
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(RunResponse {
            pipeline_id: run_id,
            status: "queued".to_string(),
            template: req.template,
            stage_count,
            created_at: Utc::now().to_rfc3339(),
        }),
    ))
}
