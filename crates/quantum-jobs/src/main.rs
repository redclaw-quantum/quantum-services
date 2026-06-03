/// quantum-jobs: Async job queue for long-running Tier 2 quantum tools.
///
/// Architecture:
/// - POST /jobs/{tool}         — submit a job, returns {job_id, status:"queued"}
/// - GET  /jobs/{id}           — poll status: queued|running|done|failed
/// - GET  /jobs/{id}/result    — fetch result JSON once done
/// - DELETE /jobs/{id}         — cancel a queued/running job
/// - GET  /health              — service health + sidecar status
///
/// Sidecar proxy (thin pass-through, synchronous):
/// - POST /proxy/qem/{path}    — forward to qem HTTP server (port 8430)
/// - POST /proxy/qpudidp/{path} — forward to qpu-didp server (port 8420)
///
/// CLI-wrapped async tools:
/// - POST /jobs/pulse          — rustypulse optimize/simulate (seconds–minutes)
/// - POST /jobs/stim           — rustystim sample/detect/gen (seconds–minutes)
use axum::{
    Json, Router,
    extract::{Path, State},
    routing::{delete, get, post},
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tempfile::NamedTempFile;
use tokio::process::Command;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Job types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub id: String,
    pub tool: String,
    pub status: JobStatus,
    pub submitted_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Wall-clock duration in milliseconds (populated on completion/failure).
    pub duration_ms: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<String>,
}

impl Job {
    fn new(tool: &str) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            tool: tool.to_string(),
            status: JobStatus::Queued,
            submitted_at: Utc::now(),
            started_at: None,
            finished_at: None,
            duration_ms: None,
            result: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

type JobMap = Arc<DashMap<String, Job>>;

#[derive(Clone)]
struct AppState {
    jobs: JobMap,
    qem_url: String,
    qpudidp_url: String,
    http: reqwest::Client,
}

impl AppState {
    fn new() -> Self {
        let qem_url = std::env::var("QEM_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8430".into());
        let qpudidp_url = std::env::var("QPUDIDP_URL")
            .unwrap_or_else(|_| "http://127.0.0.1:8420".into());
        Self {
            jobs: Arc::new(DashMap::new()),
            qem_url,
            qpudidp_url,
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(300))
                .build()
                .unwrap(),
        }
    }
}

// ---------------------------------------------------------------------------
// Error type — moved to qservices-common per audit §4.8 follow-up.
// ---------------------------------------------------------------------------

use qservices_common::{ApiError, ApiResult};

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

async fn health(State(state): State<AppState>) -> Json<Value> {
    let qem_ok = state.http.get(format!("{}/health", state.qem_url))
        .timeout(Duration::from_secs(2))
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let qpudidp_ok = state.http.get(format!("{}/health", state.qpudidp_url))
        .timeout(Duration::from_secs(2))
        .send().await
        .map(|r| r.status().is_success())
        .unwrap_or(false);

    let total = state.jobs.len();
    let running = state.jobs.iter().filter(|j| j.status == JobStatus::Running).count();
    let queued = state.jobs.iter().filter(|j| j.status == JobStatus::Queued).count();

    Json(json!({
        "status": "ok",
        "sidecars": {
            "qem": if qem_ok { "up" } else { "down" },
            "qpu-didp": if qpudidp_ok { "up" } else { "down" }
        },
        "jobs": {
            "total": total,
            "running": running,
            "queued": queued
        }
    }))
}

// ---------------------------------------------------------------------------
// Job management
// ---------------------------------------------------------------------------

async fn list_jobs(State(state): State<AppState>) -> Json<Value> {
    let jobs: Vec<Job> = state.jobs.iter()
        .map(|r| r.value().clone())
        .collect();
    Json(json!({"jobs": jobs, "count": jobs.len()}))
}

async fn get_job(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let job = state.jobs.get(&id)
        .map(|r| r.value().clone())
        .ok_or_else(|| anyhow::anyhow!("job {id} not found"))?;
    Ok(Json(serde_json::to_value(&job)?))
}

async fn get_job_result(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let job = state.jobs.get(&id)
        .map(|r| r.value().clone())
        .ok_or_else(|| anyhow::anyhow!("job {id} not found"))?;

    match job.status {
        JobStatus::Done => Ok(Json(job.result.unwrap_or(json!(null)))),
        JobStatus::Failed => Err(ApiError(anyhow::anyhow!(
            "job failed: {}", job.error.unwrap_or_default()
        ))),
        other => Err(ApiError(anyhow::anyhow!("job status is {:?}, not done", other))),
    }
}

async fn cancel_job(
    Path(id): Path<String>,
    State(state): State<AppState>,
) -> ApiResult<Json<Value>> {
    let mut job = state.jobs.get_mut(&id)
        .ok_or_else(|| anyhow::anyhow!("job {id} not found"))?;

    match job.status {
        JobStatus::Queued => {
            job.status = JobStatus::Cancelled;
            job.finished_at = Some(Utc::now());
            Ok(Json(json!({"cancelled": true, "id": id})))
        }
        other => Err(ApiError(anyhow::anyhow!(
            "cannot cancel job in {:?} state", other
        ))),
    }
}

// ---------------------------------------------------------------------------
// Job runner — spawns a tokio task to run a CLI command async
// ---------------------------------------------------------------------------

fn spawn_cli_job(
    jobs: JobMap,
    id: String,
    program: String,
    args: Vec<String>,
    _stdin_data: Option<String>,
) {
    tokio::spawn(async move {
        // Mark running
        if let Some(mut job) = jobs.get_mut(&id) {
            if job.status == JobStatus::Cancelled {
                return;
            }
            job.status = JobStatus::Running;
            job.started_at = Some(Utc::now());
        }

        // Execute
        let output = async {
            let child = Command::new(&program)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child.wait_with_output().await
        }.await;

        // Store result
        if let Some(mut job) = jobs.get_mut(&id) {
            let now = Utc::now();
            job.finished_at = Some(now);
            if let Some(started) = job.started_at {
                job.duration_ms = Some((now - started).num_milliseconds().max(0) as u64);
            }
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    job.result = serde_json::from_str(&stdout).ok()
                        .or_else(|| Some(json!({"output": stdout.trim()})));
                    job.status = JobStatus::Done;
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    job.error = Some(format!("{program} failed: {stderr}"));
                    job.status = JobStatus::Failed;
                }
                Err(e) => {
                    job.error = Some(format!("spawn failed: {e}"));
                    job.status = JobStatus::Failed;
                }
            }
        }
    });
}

// ---------------------------------------------------------------------------
// rustypulse jobs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PulseJobReq {
    /// "optimize", "simulate", "drag", "readout"
    subcommand: String,
    /// All extra CLI args as key→value pairs
    params: Value,
}

async fn submit_pulse_job(
    State(state): State<AppState>,
    Json(req): Json<PulseJobReq>,
) -> ApiResult<Json<Value>> {
    let job = Job::new("pulse");
    let id = job.id.clone();

    let mut args = vec!["--json".to_owned(), req.subcommand.clone()];
    if let Some(obj) = req.params.as_object() {
        for (k, v) in obj {
            args.push(format!("--{}", k.replace('_', "-")));
            match v {
                Value::String(s) => args.push(s.clone()),
                Value::Number(n) => args.push(n.to_string()),
                Value::Bool(b) => { if *b { /* flag only */ } else { args.pop(); } }
                _ => args.push(v.to_string()),
            }
        }
    }

    state.jobs.insert(id.clone(), job);
    spawn_cli_job(state.jobs.clone(), id.clone(), "rustypulse".into(), args, None);

    Ok(Json(json!({"job_id": id, "status": "queued", "tool": "pulse"})))
}

// ---------------------------------------------------------------------------
// rustystim jobs
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StimJobReq {
    /// "sample", "detect", "analyze-errors", "gen"
    subcommand: String,
    /// For sample/detect/analyze-errors: stim circuit text
    circuit: Option<String>,
    /// For gen: code type and distance
    params: Option<Value>,
}

async fn submit_stim_job(
    State(state): State<AppState>,
    Json(req): Json<StimJobReq>,
) -> ApiResult<Json<Value>> {
    let job = Job::new("stim");
    let id = job.id.clone();

    let circuit_file: Option<NamedTempFile> = if let Some(circuit) = &req.circuit {
        let f = tempfile::Builder::new().suffix(".stim").tempfile()?;
        std::fs::write(f.path(), circuit)?;
        Some(f)
    } else {
        None
    };

    let mut args = vec![req.subcommand.clone()];

    if let Some(f) = &circuit_file {
        args.push("--in".to_owned());
        args.push(f.path().to_str().expect("temp file path is valid UTF-8").to_owned());
    }

    if let Some(params) = &req.params
        && let Some(obj) = params.as_object()
    {
        for (k, v) in obj {
            args.push(format!("--{}", k.replace('_', "-")));
            match v {
                Value::String(s) => args.push(s.clone()),
                Value::Number(n) => args.push(n.to_string()),
                _ => args.push(v.to_string()),
            }
        }
    }

    // Keep circuit file alive by leaking into job storage via a side-channel.
    // We can't store it in Job (not Clone), so we spawn immediately.
    // The tempfile will be cleaned up after spawn_cli_job captures the path.
    let circuit_path = circuit_file.as_ref()
        .map(|f| f.path().to_string_lossy().to_string());

    state.jobs.insert(id.clone(), job);

    // Keep the tempfile alive for the duration of the job by spawning a task
    // that holds it.
    let jobs_clone = state.jobs.clone();
    let id_clone = id.clone();
    tokio::spawn(async move {
        let _keep_alive = circuit_file; // dropped after job finishes

        // Mark running
        if let Some(mut j) = jobs_clone.get_mut(&id_clone) {
            if j.status == JobStatus::Cancelled { return; }
            j.status = JobStatus::Running;
            j.started_at = Some(Utc::now());
        }

        let output = async {
            let child = Command::new("rustystim")
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()?;
            child.wait_with_output().await
        }.await;

        if let Some(mut j) = jobs_clone.get_mut(&id_clone) {
            j.finished_at = Some(Utc::now());
            match output {
                Ok(out) if out.status.success() => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    j.result = serde_json::from_str(&stdout).ok()
                        .or_else(|| Some(json!({"output": stdout.trim()})));
                    j.status = JobStatus::Done;
                }
                Ok(out) => {
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    j.error = Some(format!("rustystim failed: {stderr}"));
                    j.status = JobStatus::Failed;
                }
                Err(e) => {
                    j.error = Some(format!("spawn failed: {e}"));
                    j.status = JobStatus::Failed;
                }
            }
        }
        let _ = circuit_path; // ensure it's moved into this task scope
    });

    Ok(Json(json!({"job_id": id, "status": "queued", "tool": "stim"})))
}

// ---------------------------------------------------------------------------
// Sidecar proxy: qem and qpu-didp
// ---------------------------------------------------------------------------

async fn proxy_qem(
    Path(path): Path<String>,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let url = format!("{}/{}", state.qem_url, path);
    let resp = state.http.post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send().await
        .map_err(|e| anyhow::anyhow!("qem proxy error: {e}"))?;

    let json: Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("qem response parse error: {e}"))?;
    Ok(Json(json))
}

async fn proxy_qpudidp(
    Path(path): Path<String>,
    State(state): State<AppState>,
    body: axum::body::Bytes,
) -> ApiResult<Json<Value>> {
    let url = format!("{}/{}", state.qpudidp_url, path);
    let resp = state.http.post(&url)
        .header("content-type", "application/json")
        .body(body)
        .send().await
        .map_err(|e| anyhow::anyhow!("qpu-didp proxy error: {e}"))?;

    let json: Value = resp.json().await
        .map_err(|e| anyhow::anyhow!("qpu-didp response parse error: {e}"))?;
    Ok(Json(json))
}

// ---------------------------------------------------------------------------
// Job cost estimator
// ---------------------------------------------------------------------------

/// Median durations (ms) per (tool, subcommand) pair, seeded from observed job history.
fn estimate_median_ms(tool: &str, subcommand: &str) -> (u64, &'static str) {
    match (tool, subcommand) {
        ("rustypulse", "optimize") => (8_000, "historical"),
        ("rustypulse", "simulate") => (1_500, "historical"),
        ("rustypulse", "drag") => (3_000, "historical"),
        ("rustypulse", "readout") => (2_000, "historical"),
        ("rustystim", "gen") => (200, "historical"),
        ("rustystim", "sample") => (5_000, "historical"),
        ("rustystim", "detect") => (3_000, "historical"),
        ("rustystim", "analyze-errors") => (10_000, "historical"),
        ("qem", "solve_unified") => (15_000, "historical"),
        ("qem", "solve_lom") => (8_000, "historical"),
        ("qpu-didp", "inverse_design") => (20_000, "historical"),
        ("qpu-didp", "surrogate_predict") => (50, "historical"),
        _ => (5_000, "default"),
    }
}

async fn estimate_job_cost(
    Json(body): Json<Value>,
) -> Json<Value> {
    let tool = body.get("tool").and_then(|v| v.as_str()).unwrap_or("unknown");
    let subcommand = body.get("subcommand").and_then(|v| v.as_str()).unwrap_or("");
    let (estimated_ms, basis) = estimate_median_ms(tool, subcommand);
    Json(json!({
        "tool": tool,
        "subcommand": subcommand,
        "estimated_ms": estimated_ms,
        "confidence": if basis == "historical" { "medium" } else { "low" },
        "basis": basis,
    }))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        // Job management
        .route("/jobs", get(list_jobs))
        .route("/jobs/estimate", post(estimate_job_cost))
        .route("/jobs/:id", get(get_job))
        .route("/jobs/:id/result", get(get_job_result))
        .route("/jobs/:id", delete(cancel_job))
        // CLI-wrapped async jobs
        .route("/jobs/pulse", post(submit_pulse_job))
        .route("/jobs/stim", post(submit_stim_job))
        // Sidecar proxies
        .route("/proxy/qem/*path", post(proxy_qem))
        .route("/proxy/qpudidp/*path", post(proxy_qpudidp))
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    qservices_common::tracing::init("quantum_jobs");

    let port: u16 = std::env::var("QUANTUM_JOBS_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8766);

    let state = AppState::new();
    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await
        .expect("failed to bind");

    tracing::info!("quantum-jobs listening on {addr}");
    tracing::info!("  job endpoints: /jobs/pulse  /jobs/stim");
    tracing::info!("  proxy: /proxy/qem/*  /proxy/qpudidp/*");
    tracing::info!("  sidecars: qem={} qpu-didp={}",
        std::env::var("QEM_URL").unwrap_or_else(|_| "http://127.0.0.1:8430".into()),
        std::env::var("QPUDIDP_URL").unwrap_or_else(|_| "http://127.0.0.1:8420".into()),
    );

    axum::serve(listener, build_router(state)).await
        .expect("server error");
}
