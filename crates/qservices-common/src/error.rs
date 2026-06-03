//! JSON error envelope shared by axum services.
//!
//! Pre-extraction, this exact type lived as `struct ApiError(anyhow::Error)`
//! in *both* `quantum-api/src/main.rs` and `quantum-jobs/src/main.rs`,
//! byte-identical impls, returning `{"error": "<msg>"}` + 500.
//!
//! Behaviour is preserved 1:1 from the previous in-service definitions —
//! any handler returning `ApiResult<T>` continues to produce the same
//! response shape on the wire.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

/// Wraps `anyhow::Error` for axum handler return types.
///
/// `From<E: Into<anyhow::Error>>` means any error type that converts to
/// `anyhow::Error` (which includes every `std::error::Error`) auto-converts
/// to `ApiError`, so `?` works naturally inside handlers.
pub struct ApiError(pub anyhow::Error);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = json!({"error": self.0.to_string()});
        (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(err: E) -> Self {
        ApiError(err.into())
    }
}

/// `Result<T, ApiError>` — drop-in replacement for the per-service
/// `type ApiResult<T> = ...` aliases.
pub type ApiResult<T> = std::result::Result<T, ApiError>;
