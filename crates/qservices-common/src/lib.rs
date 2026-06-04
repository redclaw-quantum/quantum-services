//! Shared HTTP-service plumbing for the QuantumRedClaw services workspace.
//!
//! Extracted from the duplicated patterns in `quantum-api`, `quantum-jobs`,
//! and `qorchestrate-cli` per the §4.8 follow-up of
//! `/home/osobh/qclaw/quantum-consolidation-audit.md`.
//!
//! What's here:
//! - [`tracing::init`] — standard `tracing_subscriber::fmt()` boot, parameterised
//!   by service-name env-filter default.
//! - [`error::ApiError`] / [`error::ApiResult`] — the byte-identical error
//!   envelope (`{"error": "..."}` + `500 Internal Server Error`) that
//!   `quantum-api` and `quantum-jobs` were each defining locally.
//!
//! What's deliberately NOT here:
//! - **`/health` handlers** — each service returns a domain-specific payload
//!   (api probes CLI tools on PATH; jobs probes sidecar HTTP servers;
//!   qorchestrate has none today). Forcing a common shape would require
//!   a `serde_json::Value` extension point that adds no value over the
//!   per-service handlers.
//! - **CORS layer** — all three currently use `CorsLayer::permissive()`
//!   verbatim. A re-export would save one `use` line per service; not
//!   worth the indirection. Add a `cors` module here if/when the policy
//!   needs to diverge from "permissive" or vary per service.

pub mod error;
pub mod tracing;

pub use error::{ApiError, ApiResult};
