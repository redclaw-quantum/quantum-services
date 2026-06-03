use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::json;

use qorchestrate_core::{dag::DagBuilder, pipeline::PipelineDef};

use crate::server::AppState;

pub async fn handle_list_templates(State(state): State<AppState>) -> Json<serde_json::Value> {
    let mut templates = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&state.templates_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("toml")
                && let Ok(toml_str) = std::fs::read_to_string(&path)
                && let Ok(def) = PipelineDef::from_toml(&toml_str) {
                templates.push(json!({
                    "name": def.meta.name,
                    "version": def.meta.version,
                    "description": def.meta.description,
                    "stage_count": def.stages.len(),
                    "max_concurrency": def.meta.max_concurrency,
                }));
            }
        }
    }
    Json(json!({ "templates": templates }))
}

pub async fn handle_get_template(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let path = state.templates_dir.join(format!("{name}.toml"));
    match std::fs::read_to_string(&path) {
        Ok(content) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            content,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

pub async fn handle_dag(
    Path(name): Path<String>,
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let path = state.templates_dir.join(format!("{name}.toml"));
    let toml_str = std::fs::read_to_string(&path).map_err(|_| StatusCode::NOT_FOUND)?;
    let def = PipelineDef::from_toml(&toml_str).map_err(|_| StatusCode::BAD_REQUEST)?;
    let mermaid = DagBuilder::to_mermaid(&def.stages, &def.meta.name);
    Ok(Json(json!({ "mermaid": mermaid, "template": name })))
}
