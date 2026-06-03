use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use qorchestrate_core::event::StageEvent;
use qorchestrate_executor::{CheckpointStore, PipelineExecutor, PipelineRunState};

use crate::routes;

#[derive(Clone)]
pub struct AppState {
    pub executor: Arc<PipelineExecutor>,
    pub checkpoint: Arc<CheckpointStore>,
    pub templates_dir: PathBuf,
    pub brain_path: String,
    /// Live run states (run_id -> state), updated as pipelines execute.
    pub runs: Arc<RwLock<HashMap<uuid::Uuid, PipelineRunState>>>,
    /// SSE senders per run_id — handlers subscribe to these.
    pub event_channels: Arc<RwLock<HashMap<uuid::Uuid, broadcast::Sender<StageEvent>>>>,
}

pub fn build_router(state: AppState) -> axum::Router {
    use axum::routing::{delete, get, post};

    axum::Router::new()
        .route("/pipeline/run", post(routes::run::handle_run))
        .route("/pipeline/validate", post(routes::validate::handle_validate))
        .route("/pipeline/resume", post(routes::resume::handle_resume))
        .route(
            "/pipeline/templates",
            get(routes::templates::handle_list_templates),
        )
        .route(
            "/pipeline/templates/:name",
            get(routes::templates::handle_get_template),
        )
        .route(
            "/pipeline/templates/:name/dag",
            get(routes::templates::handle_dag),
        )
        .route("/pipeline/:id/status", get(routes::status::handle_status))
        .route("/pipeline/:id/stream", get(routes::stream::handle_stream))
        .route("/pipeline/:id/result", get(routes::result::handle_result))
        .route(
            "/pipeline/:id/artifacts",
            get(routes::artifacts::handle_artifacts),
        )
        .route("/pipeline/:id", delete(routes::cancel::handle_cancel))
        .layer(tower_http::cors::CorsLayer::permissive())
        .with_state(state)
}

pub async fn serve(state: AppState, port: u16) -> anyhow::Result<()> {
    let app = build_router(state);
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("qorchestrate listening on {addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request, StatusCode};
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use tower::ServiceExt;

    fn templates_dir() -> PathBuf {
        // Resolve: crates/qorchestrate-cli/src/ -> workspace root -> templates/
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("templates")
    }

    fn make_test_state() -> AppState {
        use qorchestrate_executor::{CheckpointStore, StageRegistry};
        use qorchestrate_stages::register_standard_stages;

        let mut registry = StageRegistry::new();
        register_standard_stages(&mut registry);
        let registry = Arc::new(registry);

        let checkpoint_dir = std::env::temp_dir().join(format!(
            "qorchestrate_server_test_{}",
            uuid::Uuid::new_v4()
        ));
        let checkpoint = Arc::new(CheckpointStore::new(&checkpoint_dir).unwrap());

        let executor = Arc::new(PipelineExecutor::new(
            registry,
            checkpoint.clone(),
            "http://localhost:8765",
            "http://localhost:8420",
        ));

        AppState {
            executor,
            checkpoint,
            templates_dir: templates_dir(),
            brain_path: "/tmp/test.brain".to_string(),
            runs: Arc::new(RwLock::new(HashMap::new())),
            event_channels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Compile-time test: build_router must not panic.
    #[tokio::test]
    async fn test_build_router_compiles() {
        let state = make_test_state();
        let _app = build_router(state);
        // If we reach here the router assembled without panic.
    }

    /// POST /pipeline/validate with a known template name returns valid: true.
    #[tokio::test]
    async fn test_handle_validate_valid_template() {
        let state = make_test_state();
        let app = build_router(state);

        let body = serde_json::json!({ "template": "design-to-chip" });
        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/pipeline/validate")
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(json["valid"], serde_json::Value::Bool(true));
        assert!(json["stage_count"].as_u64().unwrap_or(0) > 0);
    }

    /// GET /pipeline/templates returns at least one template entry.
    #[tokio::test]
    async fn test_handle_list_templates() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/pipeline/templates")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let templates = json["templates"].as_array().unwrap();
        assert!(
            !templates.is_empty(),
            "expected at least one template, got none"
        );
    }

    /// GET /pipeline/templates/design-to-chip/dag returns mermaid graph.
    #[tokio::test]
    async fn test_handle_dag() {
        let state = make_test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/pipeline/templates/design-to-chip/dag")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let mermaid = json["mermaid"].as_str().unwrap();
        assert!(
            mermaid.contains("graph LR"),
            "mermaid output missing 'graph LR': {mermaid}"
        );
    }
}
