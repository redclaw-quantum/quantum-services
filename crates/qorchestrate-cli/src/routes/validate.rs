use axum::{extract::State, http::StatusCode, Json};
use serde::{Deserialize, Serialize};

use qorchestrate_core::pipeline::PipelineDef;

use crate::server::AppState;

#[derive(Deserialize)]
pub struct ValidateRequest {
    pub toml: Option<String>,
    pub template: Option<String>,
}

#[derive(Serialize)]
pub struct ValidateResponse {
    pub valid: bool,
    pub pipeline_name: Option<String>,
    pub stage_count: Option<usize>,
    pub errors: Vec<String>,
}

pub async fn handle_validate(
    State(state): State<AppState>,
    Json(req): Json<ValidateRequest>,
) -> Result<Json<ValidateResponse>, (StatusCode, String)> {
    let toml_str = match (req.toml, req.template) {
        (Some(t), _) => t,
        (_, Some(name)) => {
            let path = state.templates_dir.join(format!("{name}.toml"));
            std::fs::read_to_string(&path)
                .map_err(|_| (StatusCode::NOT_FOUND, format!("Template '{name}' not found")))?
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                "Provide 'toml' or 'template'".to_string(),
            ))
        }
    };

    let def = match PipelineDef::from_toml(&toml_str) {
        Ok(d) => d,
        Err(e) => {
            return Ok(Json(ValidateResponse {
                valid: false,
                pipeline_name: None,
                stage_count: None,
                errors: vec![e.to_string()],
            }))
        }
    };

    match def.validate() {
        Ok(()) => Ok(Json(ValidateResponse {
            valid: true,
            pipeline_name: Some(def.meta.name.clone()),
            stage_count: Some(def.stages.len()),
            errors: vec![],
        })),
        Err(errs) => Ok(Json(ValidateResponse {
            valid: false,
            pipeline_name: Some(def.meta.name.clone()),
            stage_count: Some(def.stages.len()),
            errors: errs,
        })),
    }
}
