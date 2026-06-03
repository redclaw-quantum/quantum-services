//! Standard `tracing_subscriber` initialisation.
//!
//! Pre-extraction, each of `quantum-api`, `quantum-jobs`, and
//! `qorchestrate-cli` called `tracing_subscriber::fmt().with_env_filter(...).init()`
//! with a service-name-flavoured `RUST_LOG` fallback. Same boot logic,
//! three call sites.
//!
//! `init(service_name)` consolidates that. The fallback filter is
//! `"<service_name>=info,tower_http=warn"` — same shape as the
//! per-service inline defaults.
//!
//! Callers that already set `RUST_LOG` in the environment keep their
//! existing filter (the env var still wins). For services that need a
//! non-`tower_http=warn` fallback, call `init_with_default_filter`
//! instead.

use tracing_subscriber::EnvFilter;

/// Initialise tracing with a sensible service-flavoured `RUST_LOG` fallback.
///
/// `service_name` should match the `Cargo.toml` package name with `-`
/// replaced by `_` (so it's a valid env-filter target) — e.g.
/// `"quantum_api"`, `"quantum_jobs"`, `"qorchestrate"`. Idempotent calls
/// will panic because `tracing_subscriber` rejects double-init, so this
/// should be called exactly once from `main`.
pub fn init(service_name: &str) {
    let fallback = format!("{service_name}=info,tower_http=warn");
    init_with_default_filter(&fallback);
}

/// Like [`init`] but the caller supplies the full `RUST_LOG` fallback string
/// directly. Useful for services that need a non-default `tower_http` level
/// or want to track additional targets.
pub fn init_with_default_filter(fallback_filter: &str) {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new(fallback_filter)),
        )
        .init();
}
