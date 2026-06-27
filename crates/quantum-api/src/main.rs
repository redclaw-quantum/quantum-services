use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::Path,
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tempfile::NamedTempFile;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
mod qcvv;
mod cal;
mod symclaw;
use cal::{cal_health, cal_spectroscopy, cal_rabi, cal_t1, cal_rb, cal_cycle_rb, cal_adaptive, cal_leakage_rb};
use symclaw::{symclaw_health, symclaw_simplify, symclaw_differentiate, symclaw_integrate, symclaw_solve, symclaw_taylor, symclaw_limit, symclaw_codegen, symclaw_linalg, symclaw_polynomial, symclaw_analyze};
use qcvv::{qcvv_health, qcvv_quantum_volume, qcvv_process_fidelity, qcvv_zne, qcvv_clops, qcvv_rb_analysis};

// ---------------------------------------------------------------------------
// Result cache — transparent TTL cache for deterministic CLI tool calls
// ---------------------------------------------------------------------------

const CACHE_TTL: Duration = Duration::from_secs(300); // 5 minutes
const CACHE_MAX: usize = 2000;

struct CacheEntry {
    value: Value,
    inserted: Instant,
}

static RESULT_CACHE: OnceLock<Mutex<HashMap<u64, CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<HashMap<u64, CacheEntry>> {
    RESULT_CACHE.get_or_init(|| Mutex::new(HashMap::with_capacity(256)))
}

fn cache_key(program: &str, args: &[&str]) -> u64 {
    let mut h = DefaultHasher::new();
    program.hash(&mut h);
    args.hash(&mut h);
    h.finish()
}

/// Loose u64 parse: accepts a JSON integer OR a finite float (truncated).
///
/// Many integer-typed request fields (`n_qubits`, `rounds`, `n_rydberg`, …)
/// flow into these handlers from continuous-space optimizers like
/// `bayesian_outer_loop`, which produce JSON floats. The default
/// `Value::as_u64()` returns `None` for any non-integer Number, which then
/// silently falls through `.unwrap_or(default)` and erases the optimizer's
/// signal. This helper truncates floats so the value the caller wrote is
/// the value the handler sees.
fn as_u64_loose(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_f64().map(|f| f as u64))
}

/// Lookup a cached result; returns `None` if absent or expired.
fn cache_get(key: u64) -> Option<Value> {
    let guard = cache().lock().ok()?;
    let entry = guard.get(&key)?;
    if entry.inserted.elapsed() < CACHE_TTL {
        Some(entry.value.clone())
    } else {
        None
    }
}

/// Insert a result into the cache, evicting oldest entries when over limit.
fn cache_put(key: u64, value: Value) {
    let Ok(mut guard) = cache().lock() else { return };
    // Evict expired entries first; if still over limit, remove oldest.
    if guard.len() >= CACHE_MAX {
        guard.retain(|_, e| e.inserted.elapsed() < CACHE_TTL);
        if guard.len() >= CACHE_MAX {
            // Find and remove the oldest entry.
            if let Some(&oldest_key) = guard
                .iter()
                .min_by_key(|(_, e)| e.inserted)
                .map(|(k, _)| k)
            {
                guard.remove(&oldest_key);
            }
        }
    }
    guard.insert(key, CacheEntry { value, inserted: Instant::now() });
}

// claw-mesh / claw-gds (Phase 7X)
use claw_mesh::{
    TransmonCrossParams, build_transmon_cross_mesh,
    RectangularCavity3DParams, build_rectangular_cavity_3d_mesh,
    TunableTransmonMeshParams, build_tunable_transmon_mesh,
    XmonMeshParams, build_xmon_mesh,
    FluxoniumMeshParams, build_fluxonium_mesh,
    CpwResonatorMeshParams, build_cpw_resonator_mesh,
    ChipMeshConfig, build_chip_mesh,
    mesh_quality,
};
use claw_gds::{
    pcell::transmon_cross::build_transmon_cross_pcell,
    build_rectangular_cavity_3d_pcell,
    chip::{ChipConfig, build_chip_layout},
    gds_writer::write_gds,
    drc::{check_drc, DrcConfig},
    layer_map::{mapping as layer_mapping, mapping_names as layer_mapping_names, remap_cell, LayerMap},
    fabprep::{add_dummy_fill, add_tapeout_frame, FillOptions, FrameOptions},
};
use claw_gds::pcell::PCell;
use claw_tet::traits::ClawTetMesh;
use claw_yield::recipe::{
    recipe as jj_recipe, recipe_names as jj_recipe_names, tolerance_budget_with, JunctionRecipe,
};
use claw_yield::types::{NominalDesign, ProcessParams};

/// Directory containing chip design spec files.
const CHIP_DESIGNS_DIR: &str = "/nvme/quantum/data/designs";

// ---------------------------------------------------------------------------
// Error type — moved to qservices-common per audit §4.8 follow-up.
// ---------------------------------------------------------------------------

use qservices_common::{ApiError, ApiResult};

// ---------------------------------------------------------------------------
// Shell executor
// ---------------------------------------------------------------------------

/// Run a CLI tool, collect stdout JSON. Fails if exit code != 0.
///
/// Results are cached for [`CACHE_TTL`] by (program, args) key.
/// Pass `args` ending with `"--no-cache"` to bypass (the flag is stripped
/// before execution — not passed to the binary).
/// Hard limit on how long any CLI tool subprocess may run before being killed.
const TOOL_TIMEOUT: Duration = Duration::from_secs(120);

/// Spawn `program` with `args`, poll for completion, and kill it if it exceeds
/// `TOOL_TIMEOUT`.  Returns a `std::process::Output` equivalent on success.
///
/// Stdout/stderr are drained on dedicated threads *while* the child runs.
/// The previous implementation polled try_wait() first and read pipes after
/// — any tool writing more than the kernel pipe buffer (~64 KB on Linux)
/// would deadlock: the child blocked on its write, the parent slept in
/// the poll loop, and the 120 s timeout was the only escape. qexplore
/// (~400 KB JSON) tripped this consistently. Threads keep both pipes
/// drained so the child never blocks on its writes.
fn run_subprocess(
    program: &str,
    args: &[&str],
) -> Result<std::process::Output> {
    use std::io::Read as _;
    use std::process::Stdio;
    use std::thread;

    let mut child = Command::new(program)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {program} — is it on PATH?"))?;

    let stdout_handle = child.stdout.take().map(|mut s| {
        thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });
    let stderr_handle = child.stderr.take().map(|mut s| {
        thread::spawn(move || -> Vec<u8> {
            let mut buf = Vec::new();
            let _ = s.read_to_end(&mut buf);
            buf
        })
    });

    let deadline = Instant::now() + TOOL_TIMEOUT;
    let status = loop {
        match child.try_wait().context("polling subprocess")? {
            Some(s) => break s,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "{program} exceeded {:.0}s timeout and was killed",
                        TOOL_TIMEOUT.as_secs_f64()
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let stdout = stdout_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    let stderr = stderr_handle.and_then(|h| h.join().ok()).unwrap_or_default();
    Ok(std::process::Output { status, stdout, stderr })
}

fn run_tool(program: &str, args: &[&str]) -> Result<Value> {
    // Route per-domain binary names through their aggregator (post-§4.6 / §4.7
    // CLI consolidation in quantum-consolidation-audit.md). The legacy
    // `qchem` / `qopt` / `qml` binaries in quantum-apps and `qion` / `qatom` /
    // `qspin` in quantum-modalities are still built as thin shims, but the
    // canonical entry points are now `qapps <sub> ...` and
    // `qmodality <sub> ...`. Routing here keeps all call sites unchanged.
    let (program, prepended): (&str, &[&str]) = match program {
        "qchem" => ("qapps", &["chem"]),
        "qopt"  => ("qapps", &["opt"]),
        "qml"   => ("qapps", &["ml"]),
        "qion"  => ("qmodality", &["ion"]),
        "qatom" => ("qmodality", &["atom"]),
        "qspin" => ("qmodality", &["spin"]),
        other   => (other, &[]),
    };
    let routed_args: Vec<&str> =
        prepended.iter().copied().chain(args.iter().copied()).collect();
    let args: &[&str] = &routed_args;

    // Strip internal no-cache sentinel if present.
    let (bypass_cache, effective_args): (bool, Vec<&str>) = if args.last() == Some(&"--no-cache") {
        (true, args[..args.len() - 1].to_vec())
    } else {
        (false, args.to_vec())
    };

    let key = cache_key(program, &effective_args);

    if !bypass_cache && let Some(cached) = cache_get(key) {
        return Ok(cached);
    }

    let output = run_subprocess(program, &effective_args)?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("{program} failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let value: Value = serde_json::from_str(&stdout)
        .with_context(|| format!("{program} produced invalid JSON: {stdout}"))?;

    if !bypass_cache {
        cache_put(key, value.clone());
    }

    Ok(value)
}

/// Write JSON value to a temp file, return the temp file (caller keeps alive).
fn write_temp_json(value: &Value) -> Result<NamedTempFile> {
    let f = NamedTempFile::new().context("creating temp file")?;
    serde_json::to_writer(&f, value).context("writing temp JSON")?;
    Ok(f)
}

/// Send a JSON request to the symclaw-skill subprocess via stdin/stdout.
/// Kills the child if it does not respond within `TOOL_TIMEOUT`.
fn run_symclaw(request: &Value) -> Result<Value> {
    use std::io::{Read as _, Write as _};
    use std::process::Stdio;

    let mut child = Command::new("symclaw-skill")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn symclaw-skill — is it on PATH?")?;

    let req_bytes = serde_json::to_vec(request).context("serializing symclaw request")?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(&req_bytes).context("writing to symclaw-skill stdin")?;
        stdin.write_all(b"\n").context("writing newline to symclaw-skill stdin")?;
        // Drop stdin to signal EOF to the child.
    }

    let deadline = Instant::now() + TOOL_TIMEOUT;
    let status = loop {
        match child.try_wait().context("polling symclaw-skill")? {
            Some(s) => break s,
            None => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    anyhow::bail!(
                        "symclaw-skill exceeded {:.0}s timeout and was killed",
                        TOOL_TIMEOUT.as_secs_f64()
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    };

    let mut stdout_buf = Vec::new();
    let mut stderr_buf = Vec::new();
    if let Some(mut s) = child.stdout.take() { let _ = s.read_to_end(&mut stdout_buf); }
    if let Some(mut s) = child.stderr.take() { let _ = s.read_to_end(&mut stderr_buf); }

    if !status.success() {
        let stderr = String::from_utf8_lossy(&stderr_buf);
        anyhow::bail!("symclaw-skill failed: {stderr}");
    }
    let stdout = String::from_utf8_lossy(&stdout_buf);
    let value: Value = serde_json::from_str(stdout.trim())
        .with_context(|| format!("symclaw-skill produced invalid JSON: {stdout}"))?;
    Ok(value)
}

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

/// CLI tools that must be on PATH for full endpoint coverage.
///
/// Aggregator binaries `qapps` (chem/opt/ml) and `qmodality` (ion/atom/spin)
/// replaced the per-domain `qchem`/`qopt`/`qml`/`qion`/`qatom`/`qspin`
/// entries — `run_tool` routes through them automatically. The per-domain
/// binaries still exist (as 3-line shims) so the swap is reversible, but the
/// probe list reflects the canonical surface.
const REQUIRED_TOOLS: &[&str] = &[
    // Aggregators (post-§4.6 / §4.7 CLI consolidation)
    "qapps", "qmodality",
    // Stable single-binary tools
    "qtwin", "freq", "xtalk", "readout", "bench", "qstar", "surgery", "qexplore",
    "qfw", "transpile", "codesign", "pqec", "oqfp", "qcirc",
    // Pre-§6 binary names — these workspaces preserved their binary names
    // verbatim across the crate-name sweep. (rustybbq is now built — see
    // build-quantum.sh; needed for /bbq/* + /bbq/to-qcirc.)
    "rustypulse", "rustystim", "rustybbq", "rustyfloquet", "rustypkg",
    "rustycal", "rustybosonic", "rustycryo", "rustyqnet",
    "rustyextract", "rustycryo-wiring", "rustyswap", "rustyscq",
    "symclaw-skill",
];

/// Probe a tool by running `<tool> --version` (or `--help`).
/// Returns true if the binary is present and exits 0 or 1 (help exits 1 on some tools).
fn probe_tool(name: &str) -> bool {
    std::process::Command::new(name)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|_| true)   // any exit = binary exists and ran
        .unwrap_or(false) // spawn failure = not found
}

async fn health() -> Json<Value> {
    let tool_status: Vec<Value> = REQUIRED_TOOLS
        .iter()
        .map(|&t| json!({ "tool": t, "available": probe_tool(t) }))
        .collect();

    let missing: Vec<&str> = REQUIRED_TOOLS
        .iter()
        .copied()
        .filter(|&t| !probe_tool(t))
        .collect();

    let status = if missing.is_empty() { "ok" } else { "degraded" };

    Json(json!({
        "status": status,
        "tools": tool_status,
        "missing": missing,
    }))
}

// ---------------------------------------------------------------------------
// qtwin endpoints
// ---------------------------------------------------------------------------

async fn qtwin_compare(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let (design, measured) = resolve_qtwin_compare(&req);
    let design_f = write_temp_json(&design)?;
    let measured_f = write_temp_json(&measured)?;
    let result = run_tool("qtwin", &[
        "compare",
        "--design", design_f.path().to_str().unwrap(),
        "--measured", measured_f.path().to_str().unwrap(),
        "--json",
    ])?;
    Ok(Json(result))
}

/// POST /qtwin/ingest — ingest a raw cryo-measurement record, compare it against
/// the design (DesignSpec or OQFP spec), and (by default) emit recalibration
/// suggestions. Body: { measurement: <CryoMeasurementRecord>, design: <spec>,
/// recalibrate?: bool }.
/// Shared impl for `/qtwin/acquire` (record only) and `/qtwin/characterize`
/// (acquire → compare → twin + recalibration). Shells to `qtwin acquire`, which
/// runs the simulated-fridge characterization (spectroscopy/T1/T2/readout fits).
async fn qtwin_acquire_impl(req: Value, ingest: bool) -> ApiResult<Json<Value>> {
    let design = req.get("design").cloned().ok_or_else(|| {
        ApiError(anyhow::anyhow!("qtwin/acquire: missing `design` (DesignSpec or OQFP spec)"))
    })?;
    let design_f = write_temp_json(&design)?;

    let seed = req.get("seed").and_then(|v| v.as_u64()).unwrap_or(1);
    let freq_sigma = req.get("freq_sigma_mhz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let coh_sigma = req.get("coherence_sigma").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let noise = req.get("noise").and_then(|v| v.as_f64()).unwrap_or(0.015);
    let date = req.get("date").and_then(|v| v.as_str()).unwrap_or("2026-06-25").to_string();
    let offsets = req.get("freq_offsets").and_then(|v| match v {
        Value::Array(a) => Some(
            a.iter()
                .filter_map(|x| x.as_f64())
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(","),
        ),
        Value::String(s) => Some(s.clone()),
        _ => None,
    });

    let a_seed = format!("--seed={seed}");
    let a_fs = format!("--freq-sigma-mhz={freq_sigma}");
    let a_cs = format!("--coherence-sigma={coh_sigma}");
    let a_n = format!("--noise={noise}");
    let a_d = format!("--date={date}");
    let design_path = design_f.path().to_str().unwrap().to_string();
    let mut args: Vec<String> = vec![
        "acquire".into(),
        "--design".into(),
        design_path,
        a_seed,
        a_fs,
        a_cs,
        a_n,
        a_d,
        "--json".into(),
    ];
    if let Some(o) = offsets.filter(|s| !s.is_empty()) {
        args.push("--freq-offsets".into());
        args.push(o);
    }
    if ingest {
        args.push("--ingest".into());
    }
    let argref: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = run_tool("qtwin", &argref)?;
    Ok(Json(result))
}

/// POST /qtwin/acquire — run a (simulated) instrument characterization of a
/// design and return the raw `CryoMeasurementRecord`. Body: `{ design, seed?,
/// freq_sigma_mhz?, coherence_sigma?, freq_offsets?, noise?, date? }`.
async fn qtwin_acquire(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qtwin_acquire_impl(req, false).await
}

/// POST /qtwin/characterize — the full software loop: acquire → compare against
/// the design → digital twin + recalibration (closes design→fab→measure→twin).
async fn qtwin_characterize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qtwin_acquire_impl(req, true).await
}

async fn qtwin_ingest(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let measurement = req.get("measurement").cloned().ok_or_else(|| {
        ApiError(anyhow::anyhow!("qtwin/ingest: missing `measurement` (CryoMeasurementRecord)"))
    })?;
    let design = req.get("design").cloned().ok_or_else(|| {
        ApiError(anyhow::anyhow!("qtwin/ingest: missing `design` (DesignSpec or OQFP spec)"))
    })?;
    let recalibrate = req.get("recalibrate").and_then(|v| v.as_bool()).unwrap_or(true);

    let meas_f = write_temp_json(&measurement)?;
    let design_f = write_temp_json(&design)?;
    let mut args = vec![
        "ingest",
        "--measurement",
        meas_f.path().to_str().unwrap(),
        "--design",
        design_f.path().to_str().unwrap(),
        "--json",
    ];
    if recalibrate {
        args.push("--recalibrate");
    }
    let result = run_tool("qtwin", &args)?;
    Ok(Json(result))
}

async fn qtwin_recalibrate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let twin = resolve_twin_input(&req).ok_or_else(|| {
        ApiError(anyhow::anyhow!("qtwin/recalibrate: no `twin` field and no upstream stage output looks like a TwinState"))
    })?;
    let twin_f = write_temp_json(&twin)?;
    let result = run_tool("qtwin", &[
        "recalibrate",
        "--twin", twin_f.path().to_str().unwrap(),
        "--json",
    ])?;
    Ok(Json(result))
}

fn default_surface() -> String { "surface".into() }

async fn qtwin_qec_update(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let twin = resolve_twin_input(&req).ok_or_else(|| {
        ApiError(anyhow::anyhow!("qtwin/qec-update: no `twin` field and no upstream stage output looks like a TwinState"))
    })?;
    let code = req.get("code").and_then(|v| v.as_str())
        .map(|s| s.to_string()).unwrap_or_else(default_surface);
    let twin_f = write_temp_json(&twin)?;
    let result = run_tool("qtwin", &[
        "qec-update",
        "--twin", twin_f.path().to_str().unwrap(),
        "--code", &code,
        "--json",
    ])?;
    Ok(Json(result))
}

fn default_sigma() -> f64 { 15.0 }

async fn qtwin_mock(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    // Same flexibility as /qtwin/compare: accept an explicit `design`, OR
    // a DesignSpec-shaped `<dep>_output` (the full-loop template chains
    // design_phase → mock_measure; the upstream OQFP build's "design"
    // wrapper doesn't naturally surface a DesignSpec, so we fall through
    // to a synthesized rows×cols default).
    let design = req.get("design").cloned()
        .or_else(|| {
            if let Value::Object(map) = &req {
                for (_, v) in map {
                    if v.get("qubits").and_then(|q| q.as_array()).is_some()
                        && v.get("couplers").is_some() {
                        return Some(v.clone());
                    }
                }
            }
            None
        })
        .unwrap_or_else(|| {
            let rows = req.get("rows").and_then(as_u64_loose).unwrap_or(3) as usize;
            let cols = req.get("cols").and_then(as_u64_loose).unwrap_or(3) as usize;
            let base = req.get("qubit_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0);
            synthesize_design_spec(rows, cols, base)
        });
    let sigma = req.get("sigma").and_then(|v| v.as_f64()).unwrap_or_else(default_sigma);
    let design_f = write_temp_json(&design)?;
    let sigma_s = sigma.to_string();
    let result = run_tool("qtwin", &[
        "mock",
        "--design", design_f.path().to_str().unwrap(),
        "--sigma", &sigma_s,
        "--json",
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// freq endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct FreqOptimizeReq {
    /// Built-in topology name OR omit with topology_json
    topology: Option<String>,
    /// Inline topology JSON (alternative to named topology)
    topology_json: Option<Value>,
    #[serde(default = "default_3")]
    rows: usize,
    #[serde(default = "default_3")]
    cols: usize,
    #[serde(default = "default_5")]
    n: usize,
    #[serde(default = "default_greedy")]
    optimizer: String,
}

fn default_3() -> usize { 3 }
fn default_5() -> usize { 5 }
fn default_greedy() -> String { "greedy".into() }

/// Normalize the various `optimizer` aliases templates use (the freq CLI
/// only accepts the short forms `greedy` and `sa`, but design-to-chip
/// passes the full `simulated_annealing`).
fn normalize_freq_optimizer(s: &str) -> &'static str {
    match s {
        "sa" | "simulated_annealing" | "sim_annealing" | "annealing" => "sa",
        _ => "greedy",
    }
}

async fn freq_optimize(
    Json(req): Json<FreqOptimizeReq>,
) -> ApiResult<Json<Value>> {
    let args: Vec<String> = vec!["--json".into(), "optimize".into()];
    let optimizer = normalize_freq_optimizer(&req.optimizer);

    if let Some(topo_json) = &req.topology_json {
        let f = write_temp_json(topo_json)?;
        // Keep file alive during run
        let path = f.path().to_str().unwrap().to_string();
        let mut tool_args: Vec<&str> = vec!["--json", "optimize", "--topology-file", &path,
            "--optimizer", optimizer];
        let rows_s = req.rows.to_string();
        let cols_s = req.cols.to_string();
        let n_s = req.n.to_string();
        tool_args.extend_from_slice(&["--rows", &rows_s, "--cols", &cols_s, "--n", &n_s]);
        let result = run_tool("freq", &tool_args)?;
        return Ok(Json(result));
    }

    let topology = req.topology.unwrap_or_else(|| "heavy_hex".into());
    let rows_s = req.rows.to_string();
    let cols_s = req.cols.to_string();
    let n_s = req.n.to_string();

    let _ = args; // consumed above
    let result = run_tool("freq", &[
        "--json", "optimize",
        "--topology", &topology,
        "--rows", &rows_s,
        "--cols", &cols_s,
        "--n", &n_s,
        "--optimizer", optimizer,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct FreqCheckReq {
    topology: Value,
    assignments: Value,
}

async fn freq_check(
    Json(req): Json<FreqCheckReq>,
) -> ApiResult<Json<Value>> {
    let topo_f = write_temp_json(&req.topology)?;
    let assign_f = write_temp_json(&req.assignments)?;
    let result = run_tool("freq", &[
        "--json", "check",
        "--topology-file", topo_f.path().to_str().unwrap(),
        "--assignment-file", assign_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct FreqYieldReq {
    topology: Option<String>,
    topology_json: Option<Value>,
    #[serde(default = "default_3")]
    rows: usize,
    #[serde(default = "default_3")]
    cols: usize,
    #[serde(default = "default_5")]
    n: usize,
    #[serde(default = "default_sigma")]
    sigma: f64,
    #[serde(default = "default_10000")]
    samples: usize,
}

fn default_10000() -> usize { 10000 }

async fn freq_yield(
    Json(req): Json<FreqYieldReq>,
) -> ApiResult<Json<Value>> {
    let sigma_s = req.sigma.to_string();
    let samples_s = req.samples.to_string();
    let rows_s = req.rows.to_string();
    let cols_s = req.cols.to_string();
    let n_s = req.n.to_string();

    if let Some(topo_json) = &req.topology_json {
        let f = write_temp_json(topo_json)?;
        let path = f.path().to_str().unwrap().to_string();
        let result = run_tool("freq", &[
            "--json", "yield",
            "--topology-file", &path,
            "--sigma", &sigma_s,
            "--samples", &samples_s,
        ])?;
        return Ok(Json(result));
    }

    let topology = req.topology.unwrap_or_else(|| "heavy_hex".into());
    let result = run_tool("freq", &[
        "--json", "yield",
        "--topology", &topology,
        "--rows", &rows_s, "--cols", &cols_s, "--n", &n_s,
        "--sigma", &sigma_s,
        "--samples", &samples_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// xtalk endpoints
// ---------------------------------------------------------------------------

/// Build a ChipLayout JSON suitable for the `xtalk` binary from rows×cols
/// grid params, plus an optional list of per-qubit frequency overrides.
/// Couplers connect each qubit to its right and bottom neighbors.
///
/// `freq_overrides` is the shape `/freq/optimize` returns — a flat array of
/// `{qubit_id, assigned_freq_ghz, …}` entries. The fallback when no override
/// is found for a given qubit_id is `qubit_freq_ghz` with a 0.5 GHz
/// alternation between odd/even sites (matches `xtalk_zz_simple`).
fn synthesize_chip_layout(
    rows: usize,
    cols: usize,
    qubit_freq_ghz: f64,
    coupling_g_mhz: f64,
    freq_overrides: Option<&Vec<Value>>,
) -> Value {
    let mut qubits = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let id = r * cols + c;
            let default_freq = qubit_freq_ghz + if (r + c) % 2 == 0 { 0.0 } else { 0.5 };
            let freq = freq_overrides
                .and_then(|arr| {
                    arr.iter().find_map(|entry| {
                        let entry_id = entry
                            .get("qubit_id")
                            .and_then(|v| v.as_u64())
                            .map(|n| n as usize);
                        if entry_id == Some(id) {
                            entry.get("assigned_freq_ghz").and_then(|v| v.as_f64())
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or(default_freq);
            qubits.push(serde_json::json!({
                "id": id,
                "device_type": "fixed_transmon",
                "frequency_ghz": freq,
                "anharmonicity_mhz": -200.0,
                "position": [c as f64 * 500.0, r as f64 * 500.0]
            }));
        }
    }
    let mut couplers = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let id = r * cols + c;
            if c + 1 < cols {
                couplers.push(serde_json::json!({
                    "qubit_a": id, "qubit_b": id + 1,
                    "coupling_g_mhz": coupling_g_mhz,
                    "coupler_type": {"type": "direct"}
                }));
            }
            if r + 1 < rows {
                couplers.push(serde_json::json!({
                    "qubit_a": id, "qubit_b": id + cols,
                    "coupling_g_mhz": coupling_g_mhz,
                    "coupler_type": {"type": "direct"}
                }));
            }
        }
    }
    serde_json::json!({"qubits": qubits, "couplers": couplers})
}

/// Resolve a ChipLayout for the xtalk endpoints. Priority:
///   1. Explicit `layout` in the body (canonical, existing behaviour).
///   2. Auto-synthesize from `rows`/`cols`/`qubit_freq_ghz`/`coupling_g_mhz`
///      (the `/xtalk/zz-simple` shape).
///   3. Auto-synthesize while honouring upstream `freq_opt_output` — a list
///      of `{qubit_id, assigned_freq_ghz, …}` from `/freq/optimize`. Any
///      `<dep>_output` whose value is such a list is taken as the override.
///
/// Lets orchestrate templates wire `freq_optimize → xtalk_analyze` without
/// constructing a `ChipLayout` themselves.
fn resolve_xtalk_layout(req: &Value) -> Value {
    if let Some(layout) = req.get("layout").cloned() {
        return layout;
    }
    let rows = req.get("rows").and_then(as_u64_loose).unwrap_or(3) as usize;
    let cols = req.get("cols").and_then(as_u64_loose).unwrap_or(3) as usize;
    let qubit_freq_ghz = req
        .get("qubit_freq_ghz")
        .or_else(|| req.get("qubit_freq"))
        .and_then(|v| v.as_f64())
        .unwrap_or(5.0);
    let coupling_g_mhz = req
        .get("coupling_g_mhz")
        .and_then(|v| v.as_f64())
        .unwrap_or(3.0);
    let freq_overrides: Option<Vec<Value>> = if let Value::Object(map) = req {
        map.iter().find_map(|(k, v)| {
            if !k.ends_with("_output") { return None; }
            v.as_array().filter(|arr| {
                arr.first().is_some_and(|e|
                    e.get("qubit_id").is_some() && e.get("assigned_freq_ghz").is_some()
                )
            }).cloned()
        })
    } else { None };
    synthesize_chip_layout(rows, cols, qubit_freq_ghz, coupling_g_mhz, freq_overrides.as_ref())
}

// ─── qtwin synthesizers ────────────────────────────────────────────────────
//
// `chip-to-calibration.toml` and related templates start with `twin_compare`
// and expect `{design, measured}` to be supplied by the caller. For smoke
// testing — and for any pipeline that wants a sanity-check run without real
// hardware — synthesize both from the same rows×cols / base-frequency knobs
// that the rest of the SC pipeline already uses. Real callers can still pass
// explicit `design` / `measured` fields and bypass synthesis entirely.

/// Build a minimal valid `DesignSpec` JSON for a rows×cols heavy-hex array.
fn synthesize_design_spec(rows: usize, cols: usize, base_freq_ghz: f64) -> Value {
    let mut qubits = Vec::with_capacity(rows * cols);
    for r in 0..rows {
        for c in 0..cols {
            let id = r * cols + c;
            let freq = base_freq_ghz + if (r + c) % 2 == 0 { 0.0 } else { 0.5 };
            qubits.push(json!({
                "id": id,
                "target_freq_ghz": freq,
                "predicted_t1_us": 80.0,
                "predicted_t2_us": 60.0,
                "anharmonicity_mhz": -200.0,
                "geometry": [c as f64 * 500.0, r as f64 * 500.0]
            }));
        }
    }
    let mut couplers = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let id = r * cols + c;
            if c + 1 < cols {
                couplers.push(json!({
                    "qubit_a": id, "qubit_b": id + 1,
                    "target_coupling_mhz": 3.0, "target_zz_khz": 50.0
                }));
            }
            if r + 1 < rows {
                couplers.push(json!({
                    "qubit_a": id, "qubit_b": id + cols,
                    "target_coupling_mhz": 3.0, "target_zz_khz": 50.0
                }));
            }
        }
    }
    json!({ "qubits": qubits, "couplers": couplers })
}

/// Build a `MeasuredSpec` JSON that mirrors a `DesignSpec` with no drift.
/// (Used when the template doesn't supply measured data — keeps deviation
/// counts at zero so the conditional `pulse_retune` stage is skipped.)
fn synthesize_measured_from_design(design: &Value) -> Value {
    let qubits: Vec<Value> = design
        .get("qubits").and_then(|v| v.as_array()).cloned().unwrap_or_default()
        .iter()
        .map(|q| json!({
            "id": q.get("id").cloned().unwrap_or(json!(0)),
            "measured_freq_ghz": q.get("target_freq_ghz").cloned().unwrap_or(json!(5.0)),
            "measured_t1_us": q.get("predicted_t1_us").cloned().unwrap_or(json!(80.0)),
            "measured_t2_us": q.get("predicted_t2_us").cloned().unwrap_or(json!(60.0)),
            "measured_anharmonicity_mhz": q.get("anharmonicity_mhz").cloned().unwrap_or(json!(-200.0)),
            "readout_fidelity": 0.99
        }))
        .collect();
    let couplers: Vec<Value> = design
        .get("couplers").and_then(|v| v.as_array()).cloned().unwrap_or_default()
        .iter()
        .map(|c| json!({
            "qubit_a": c.get("qubit_a").cloned().unwrap_or(json!(0)),
            "qubit_b": c.get("qubit_b").cloned().unwrap_or(json!(1)),
            "measured_coupling_mhz": c.get("target_coupling_mhz").cloned().unwrap_or(json!(3.0)),
            "measured_zz_khz": c.get("target_zz_khz").cloned().unwrap_or(json!(50.0)),
            "gate_fidelity": 0.999
        }))
        .collect();
    json!({
        "qubits": qubits,
        "couplers": couplers,
        "measurement_date": "2026-01-01T00:00:00Z"
    })
}

/// Resolve `(design, measured)` for `/qtwin/compare`. Priority:
///   1. Explicit `design` + `measured` fields in the body.
///   2. Explicit `design`, auto-synthesize matching `measured`.
///   3. Auto-synthesize both from rows×cols (+ optional `qubit_freq_ghz`).
fn resolve_qtwin_compare(req: &Value) -> (Value, Value) {
    let design = req.get("design").cloned().unwrap_or_else(|| {
        let rows = req.get("rows").and_then(as_u64_loose).unwrap_or(3) as usize;
        let cols = req.get("cols").and_then(as_u64_loose).unwrap_or(3) as usize;
        let base = req.get("qubit_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0);
        synthesize_design_spec(rows, cols, base)
    });
    let measured = req.get("measured").cloned()
        .unwrap_or_else(|| synthesize_measured_from_design(&design));
    (design, measured)
}

/// Resolve a `TwinState`-shaped value for `/qtwin/recalibrate` and
/// `/qtwin/qec-update`. Priority:
///   1. Explicit `twin` field.
///   2. Any `<dep>_output` whose value looks like a TwinState
///      (has both `deviations` and `overall_status`).
fn resolve_twin_input(req: &Value) -> Option<Value> {
    if let Some(twin) = req.get("twin").cloned() { return Some(twin); }
    if let Value::Object(map) = req {
        for (k, v) in map {
            if !k.ends_with("_output") { continue; }
            if v.get("deviations").is_some() && v.get("overall_status").is_some() {
                return Some(v.clone());
            }
        }
    }
    None
}

async fn xtalk_coupling(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let layout = resolve_xtalk_layout(&req);
    let layout_f = write_temp_json(&layout)?;
    let result = run_tool("xtalk", &[
        "coupling",
        "--layout", layout_f.path().to_str().unwrap(),
        "--json",
    ])?;
    Ok(Json(result))
}

async fn xtalk_zz(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let layout = resolve_xtalk_layout(&req);
    let layout_f = write_temp_json(&layout)?;
    let result = run_tool("xtalk", &[
        "zz",
        "--layout", layout_f.path().to_str().unwrap(),
        "--json",
    ])?;
    Ok(Json(result))
}

async fn xtalk_crosstalk(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let layout = resolve_xtalk_layout(&req);
    let layout_f = write_temp_json(&layout)?;
    let drive = req
        .get("drive_qubit")
        .and_then(as_u64_loose)
        .unwrap_or(0)
        .to_string();
    let result = run_tool("xtalk", &[
        "crosstalk",
        "--layout", layout_f.path().to_str().unwrap(),
        "--drive-qubit", &drive,
        "--json",
    ])?;
    Ok(Json(result))
}

async fn xtalk_simulate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let layout = resolve_xtalk_layout(&req);
    let layout_f = write_temp_json(&layout)?;
    let gate = req
        .get("gate")
        .and_then(|v| v.as_str())
        .unwrap_or("cx")
        .to_string();
    let qubits = req
        .get("qubits")
        .and_then(|v| v.as_str())
        .unwrap_or("0,1")
        .to_string();
    let result = run_tool("xtalk", &[
        "simulate",
        "--layout", layout_f.path().to_str().unwrap(),
        "--gate", &gate,
        "--qubits", &qubits,
        "--json",
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// readout endpoints
// ---------------------------------------------------------------------------

fn default_anharmonicity() -> f64 { -250.0 }
fn default_fidelity() -> f64 { 0.999 }

/// Resolve a Hamiltonian target `(qubit_freq_ghz, anharmonicity_mhz)` from a
/// flexible request body. Used by handlers that previously required strict
/// fields but are now called from orchestrate pipelines where upstream stage
/// output (e.g. inverse_design) is the natural source.
fn resolve_hamiltonian_target(req: &Value) -> (f64, f64) {
    // Accept all three HamiltonianParams field-name conventions (see
    // quantum-gaps/SHARED-IR-AUDIT.md): `qubit_freq[_ghz]` (quantum-services),
    // `qubit_frequency_ghz` (qem-core), `qubit_frequency` (qpu-didp-core newtype).
    let freq_of = |o: &Value| {
        o.get("qubit_freq").and_then(|v| v.as_f64())
            .or_else(|| o.get("qubit_freq_ghz").and_then(|v| v.as_f64()))
            .or_else(|| o.get("qubit_frequency_ghz").and_then(|v| v.as_f64()))
            .or_else(|| o.get("qubit_frequency").and_then(|v| v.as_f64()))
    };
    let anh_of = |o: &Value| {
        o.get("anharmonicity").and_then(|v| v.as_f64())
            .or_else(|| o.get("anharmonicity_mhz").and_then(|v| v.as_f64()))
    };
    let direct = freq_of(req);
    let direct_anh = anh_of(req);
    if let (Some(f), Some(a)) = (direct, direct_anh) {
        return (f, a);
    }
    // Look at upstream stage outputs for a FlowDesignResponse-shaped value.
    if let Value::Object(map) = req {
        for (k, v) in map {
            if !k.ends_with("_output") { continue; }
            if let Some(h) = v.get("best_candidate").and_then(|c| c.get("predicted_hamiltonian"))
                && let (Some(f), Some(a)) = (freq_of(h), anh_of(h))
            {
                return (f, a);
            }
        }
    }
    (direct.unwrap_or(5.0), direct_anh.unwrap_or(default_anharmonicity()))
}

async fn readout_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let (qubit_freq, anharmonicity) = resolve_hamiltonian_target(&req);
    let target_fidelity = req.get("target_fidelity").and_then(|v| v.as_f64())
        .unwrap_or_else(default_fidelity);
    let freq_s = qubit_freq.to_string();
    let anh_s = anharmonicity.to_string();
    let fid_s = target_fidelity.to_string();
    let result = run_tool("readout", &[
        "--json", "design",
        "--qubit-freq", &freq_s,
        "--anharmonicity", &anh_s,
        "--target-fidelity", &fid_s,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutMultiplexReq {
    /// Comma-separated GHz values or array
    qubit_freqs: Value,
    #[serde(default = "default_1")]
    feedlines: usize,
}

fn default_1() -> usize { 1 }

async fn readout_multiplex(
    Json(req): Json<ReadoutMultiplexReq>,
) -> ApiResult<Json<Value>> {
    let freqs_str = match &req.qubit_freqs {
        Value::String(s) => s.clone(),
        Value::Array(arr) => arr.iter()
            .filter_map(|v| v.as_f64())
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .join(","),
        _ => return Err(ApiError(anyhow::anyhow!("qubit_freqs must be a string or array of numbers"))),
    };
    let feedlines_s = req.feedlines.to_string();
    let result = run_tool("readout", &[
        "--json", "multiplex",
        "--qubit-freqs", &freqs_str,
        "--feedlines", &feedlines_s,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutOptimizeReq {
    qubit_freq: f64,
    t1: f64,
    #[serde(default = "default_fidelity")]
    target_fidelity: f64,
}

async fn readout_optimize(
    Json(req): Json<ReadoutOptimizeReq>,
) -> ApiResult<Json<Value>> {
    let freq_s = req.qubit_freq.to_string();
    let t1_s = req.t1.to_string();
    let fid_s = req.target_fidelity.to_string();
    let result = run_tool("readout", &[
        "--json", "optimize",
        "--qubit-freq", &freq_s,
        "--t1", &t1_s,
        "--target-fidelity", &fid_s,
    ])?;
    Ok(Json(result))
}

async fn readout_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "readout"}))
}

#[derive(Deserialize)]
struct ReadoutFidelityReq {
    chi: f64,
    kappa: f64,
    t1: f64,
    integration_time: f64,
}

async fn readout_fidelity(
    Json(req): Json<ReadoutFidelityReq>,
) -> ApiResult<Json<Value>> {
    let chi_s = req.chi.to_string();
    let kappa_s = req.kappa.to_string();
    let t1_s = req.t1.to_string();
    let int_s = req.integration_time.to_string();
    let result = run_tool("readout", &[
        "--json", "fidelity",
        "--chi", &chi_s,
        "--kappa", &kappa_s,
        "--t1", &t1_s,
        "--integration-time", &int_s,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct XtalkTunableCouplerReq {
    freq_a: f64,
    freq_b: f64,
    #[serde(default = "default_anharm_neg")]
    alpha_a: f64,
    #[serde(default = "default_anharm_neg")]
    alpha_b: f64,
    g_ac: f64,
    g_bc: f64,
    #[serde(default)]
    g_direct: f64,
    #[serde(default = "default_coupler_anharm")]
    coupler_anharmonicity: f64,
    search_lo: f64,
    search_hi: f64,
    #[serde(default = "default_zz_levels")]
    n_levels: usize,
}
fn default_anharm_neg() -> f64 { -250.0 }
fn default_coupler_anharm() -> f64 { -200.0 }
fn default_zz_levels() -> usize { 4 }

/// Tunable-coupler ZZ-cancellation: decoupling (g_eff=0) and exact ZZ=0 bias points.
async fn xtalk_tunable_coupler(Json(req): Json<XtalkTunableCouplerReq>) -> ApiResult<Json<Value>> {
    let (fa, fb, aa, ab, gac, gbc, gd, ca, lo, hi, nl) = (
        req.freq_a.to_string(),
        req.freq_b.to_string(),
        req.alpha_a.to_string(),
        req.alpha_b.to_string(),
        req.g_ac.to_string(),
        req.g_bc.to_string(),
        req.g_direct.to_string(),
        req.coupler_anharmonicity.to_string(),
        req.search_lo.to_string(),
        req.search_hi.to_string(),
        req.n_levels.to_string(),
    );
    let result = run_tool(
        "xtalk",
        &[
            "--json",
            "tunable-coupler",
            "--freq-a", &fa,
            "--freq-b", &fb,
            "--alpha-a", &aa,
            "--alpha-b", &ab,
            "--g-ac", &gac,
            "--g-bc", &gbc,
            "--g-direct", &gd,
            "--coupler-anharmonicity", &ca,
            "--search-lo", &lo,
            "--search-hi", &hi,
            "--n-levels", &nl,
        ],
    )?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutMistReq {
    chi: f64,
    kappa: f64,
    #[serde(default = "default_detuning")]
    detuning: f64,
    #[serde(default = "default_anharm")]
    anharmonicity: f64,
    #[serde(default = "default_qnd_target")]
    qnd_target: f64,
}
fn default_detuning() -> f64 { 1500.0 }
fn default_anharm() -> f64 { 200.0 }
fn default_qnd_target() -> f64 { 0.01 }

/// Measurement-induced state transition (MIST) photon ceiling for dispersive readout.
async fn readout_mist(Json(req): Json<ReadoutMistReq>) -> ApiResult<Json<Value>> {
    let (chi, kappa, det, anh, qnd) = (
        req.chi.to_string(),
        req.kappa.to_string(),
        req.detuning.to_string(),
        req.anharmonicity.to_string(),
        req.qnd_target.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "mist",
        "--chi", &chi, "--kappa", &kappa,
        "--detuning", &det, "--anharmonicity", &anh, "--qnd-target", &qnd,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutErasureReq {
    t1: f64,
    chi: f64,
    kappa: f64,
    #[serde(default = "default_check_window")]
    check_window: f64,
    #[serde(default = "default_p_pauli")]
    p_pauli: f64,
    #[serde(default = "default_erasure_photons")]
    n_photons: f64,
}
fn default_check_window() -> f64 { 400.0 }
fn default_p_pauli() -> f64 { 1e-4 }
fn default_erasure_photons() -> f64 { 5.0 }

/// Dual-rail erasure-qubit analysis + fault-tolerance threshold advantage.
async fn readout_erasure(Json(req): Json<ReadoutErasureReq>) -> ApiResult<Json<Value>> {
    let (t1, cw, chi, kappa, pp, np) = (
        req.t1.to_string(),
        req.check_window.to_string(),
        req.chi.to_string(),
        req.kappa.to_string(),
        req.p_pauli.to_string(),
        req.n_photons.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "erasure",
        "--t1", &t1, "--check-window", &cw,
        "--chi", &chi, "--kappa", &kappa,
        "--p-pauli", &pp, "--n-photons", &np,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutParampReq {
    #[serde(default = "default_paramp_kind")]
    kind: String,
    #[serde(default = "default_paramp_gain")]
    gain: f64,
    #[serde(default = "default_paramp_freq")]
    freq: f64,
    #[serde(default = "default_paramp_saturation")]
    saturation: f64,
    #[serde(default = "default_paramp_bandwidth")]
    bandwidth: f64,
    #[serde(default = "default_paramp_phase_noise")]
    phase_noise: f64,
    #[serde(default = "default_paramp_signal")]
    signal_power: f64,
}
fn default_paramp_kind() -> String { "jpa".to_string() }
fn default_paramp_gain() -> f64 { 20.0 }
fn default_paramp_freq() -> f64 { 6.0 }
fn default_paramp_saturation() -> f64 { -100.0 }
fn default_paramp_bandwidth() -> f64 { 500.0 }
fn default_paramp_phase_noise() -> f64 { -110.0 }
fn default_paramp_signal() -> f64 { -130.0 }

/// Parametric first-stage amplifier: gain compression, added noise, pump.
async fn readout_paramp(Json(req): Json<ReadoutParampReq>) -> ApiResult<Json<Value>> {
    let (gain, freq, sat, bw, pn, sig) = (
        req.gain.to_string(),
        req.freq.to_string(),
        req.saturation.to_string(),
        req.bandwidth.to_string(),
        req.phase_noise.to_string(),
        req.signal_power.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "paramp",
        "--kind", &req.kind,
        "--gain", &gain, "--freq", &freq,
        "--saturation", &sat, "--bandwidth", &bw,
        "--phase-noise", &pn, "--signal-power", &sig,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutResetReq {
    #[serde(default = "default_reset_kind")]
    kind: String,
    #[serde(default = "default_reset_p0")]
    p0: f64,
    #[serde(default = "default_reset_efficiency")]
    efficiency: f64,
    #[serde(default = "default_reset_rounds")]
    rounds: u32,
    #[serde(default = "default_reset_floor")]
    thermal_floor: f64,
    #[serde(default = "default_reset_meas")]
    meas_time: f64,
    #[serde(default = "default_reset_drive")]
    drive_time: f64,
}
fn default_reset_kind() -> String { "feedback".to_string() }
fn default_reset_p0() -> f64 { 0.1 }
fn default_reset_efficiency() -> f64 { 0.9 }
fn default_reset_rounds() -> u32 { 3 }
fn default_reset_floor() -> f64 { 1e-3 }
fn default_reset_meas() -> f64 { 200.0 }
fn default_reset_drive() -> f64 { 50.0 }

/// Active/fast qubit reset: residual excited-state population vs feedback rounds.
async fn readout_reset(Json(req): Json<ReadoutResetReq>) -> ApiResult<Json<Value>> {
    let (p0, eff, rounds, floor, meas, drive) = (
        req.p0.to_string(),
        req.efficiency.to_string(),
        req.rounds.to_string(),
        req.thermal_floor.to_string(),
        req.meas_time.to_string(),
        req.drive_time.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "reset",
        "--kind", &req.kind,
        "--p0", &p0, "--efficiency", &eff, "--rounds", &rounds,
        "--thermal-floor", &floor, "--meas-time", &meas, "--drive-time", &drive,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutFeedforwardReq {
    #[serde(default = "default_ff_meas")]
    meas_time: f64,
    #[serde(default = "default_ff_classical")]
    classical_latency: f64,
    #[serde(default = "default_ff_apply")]
    apply_time: f64,
    #[serde(default = "default_ff_t2")]
    spectator_t2: f64,
    #[serde(default = "default_ff_assign")]
    assignment_error: f64,
    #[serde(default = "default_ff_t1")]
    t1: f64,
}
fn default_ff_meas() -> f64 { 400.0 }
fn default_ff_classical() -> f64 { 100.0 }
fn default_ff_apply() -> f64 { 50.0 }
fn default_ff_t2() -> f64 { 50.0 }
fn default_ff_assign() -> f64 { 0.01 }
fn default_ff_t1() -> f64 { 50.0 }

/// Mid-circuit measurement + feedforward latency and error budget.
async fn readout_feedforward(Json(req): Json<ReadoutFeedforwardReq>) -> ApiResult<Json<Value>> {
    let (meas, cl, apply, t2, assign, t1) = (
        req.meas_time.to_string(),
        req.classical_latency.to_string(),
        req.apply_time.to_string(),
        req.spectator_t2.to_string(),
        req.assignment_error.to_string(),
        req.t1.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "feedforward",
        "--meas-time", &meas, "--classical-latency", &cl, "--apply-time", &apply,
        "--spectator-t2", &t2, "--assignment-error", &assign, "--t1", &t1,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct ReadoutLeakageReq {
    #[serde(default = "default_leak_photons")]
    n_photons: f64,
    #[serde(default = "default_leak_coupling")]
    nonlinear_coupling: f64,
    #[serde(default = "default_leak_detuning")]
    detuning: f64,
    #[serde(default = "default_leak_time")]
    readout_time: f64,
}
fn default_leak_photons() -> f64 { 5.0 }
fn default_leak_coupling() -> f64 { 20.0 }
fn default_leak_detuning() -> f64 { 1500.0 }
fn default_leak_time() -> f64 { 500.0 }

/// Readout-induced leakage (nonlinear coupling) — distinct from MIST.
async fn readout_leakage(Json(req): Json<ReadoutLeakageReq>) -> ApiResult<Json<Value>> {
    let (np, nl, det, rt) = (
        req.n_photons.to_string(),
        req.nonlinear_coupling.to_string(),
        req.detuning.to_string(),
        req.readout_time.to_string(),
    );
    let result = run_tool("readout", &[
        "--json", "leakage",
        "--n-photons", &np, "--nonlinear-coupling", &nl,
        "--detuning", &det, "--readout-time", &rt,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// bench endpoints
// ---------------------------------------------------------------------------

/// Build a ChipSpec JSON from uniform parameters.
/// Accepts either a full `chip_spec` object or individual scalar params.
fn make_chip_spec(req: &BenchReq) -> Value {
    if let Some(spec) = &req.chip_spec {
        return spec.clone();
    }
    let n = req.n_qubits.unwrap_or(20);
    let t1 = req.t1.unwrap_or(80.0);
    let t2 = req.t2.unwrap_or(60.0);
    let gf = req.gate_fidelity.unwrap_or(0.9987);
    let rf = req.readout_fidelity.unwrap_or(0.997);
    let sq = req.single_q_fidelity.unwrap_or(0.9995);
    let gate_time = req.gate_time_ns.unwrap_or(35.0);

    // Build linear coupling map
    let coupling: Vec<[usize; 2]> = (0..n - 1).map(|i| [i, i + 1]).collect();
    // Build gate_fidelities as [[a,b,f], ...] format expected by serde
    let gate_fid: Vec<Value> = (0..n - 1)
        .map(|i| json!([[i, i + 1], gf]))
        .collect();
    let t1s: Vec<f64> = vec![t1; n];
    let t2s: Vec<f64> = vec![t2; n];
    let rfs: Vec<f64> = vec![rf; n];
    let sqs: Vec<f64> = vec![sq; n];

    json!({
        "num_qubits": n,
        "coupling_map": coupling,
        "gate_fidelities": gate_fid,
        "single_q_fidelities": sqs,
        "t1_us": t1s,
        "t2_us": t2s,
        "readout_fidelities": rfs,
        "gate_time_ns": gate_time
    })
}

#[derive(Deserialize)]
struct BenchReq {
    /// Full ChipSpec JSON (takes priority)
    chip_spec: Option<Value>,
    /// Uniform param shorthand
    n_qubits: Option<usize>,
    t1: Option<f64>,
    t2: Option<f64>,
    gate_fidelity: Option<f64>,
    readout_fidelity: Option<f64>,
    single_q_fidelity: Option<f64>,
    gate_time_ns: Option<f64>,
    #[serde(default = "default_1000")]
    trials: usize,
}

async fn bench_predict(
    Json(req): Json<BenchReq>,
) -> ApiResult<Json<Value>> {
    let spec = make_chip_spec(&req);
    let chip_f = write_temp_json(&spec)?;
    let trials_s = req.trials.to_string();
    let result = run_tool("bench", &[
        "--json", "predict",
        "--chip", chip_f.path().to_str().unwrap(),
        "--trials", &trials_s,
    ])?;
    Ok(Json(result))
}

async fn bench_suggest(
    Json(req): Json<BenchReq>,
) -> ApiResult<Json<Value>> {
    let spec = make_chip_spec(&req);
    let chip_f = write_temp_json(&spec)?;
    let result = run_tool("bench", &[
        "--json", "suggest",
        "--chip", chip_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// GET /bench/health
async fn bench_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "bench"}))
}

/// POST /bench/qv — estimate Quantum Volume.
async fn bench_qv(Json(req): Json<BenchReq>) -> ApiResult<Json<Value>> {
    let spec = make_chip_spec(&req);
    let chip_f = write_temp_json(&spec)?;
    let trials_s = req.trials.to_string();
    let result = run_tool("bench", &[
        "--json", "qv",
        "--chip", chip_f.path().to_str().unwrap(),
        "--trials", &trials_s,
    ])?;
    Ok(Json(result))
}

/// POST /bench/rb — Randomized Benchmarking for a single qubit.
///
/// Accepts same chip params as /bench/predict plus `qubit` (index) and
/// `max_depth` (default 100).
async fn bench_rb(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_qubits = req.get("n_qubits").and_then(as_u64_loose).unwrap_or(20) as usize;
    let t1 = req.get("t1").and_then(|v| v.as_f64()).unwrap_or(80.0);
    let t2 = req.get("t2").and_then(|v| v.as_f64()).unwrap_or(60.0);
    let gf = req.get("gate_fidelity").and_then(|v| v.as_f64()).unwrap_or(0.9987);
    let rf = req.get("readout_fidelity").and_then(|v| v.as_f64()).unwrap_or(0.997);
    let sq = req.get("single_q_fidelity").and_then(|v| v.as_f64()).unwrap_or(0.9995);
    let gate_time = req.get("gate_time_ns").and_then(|v| v.as_f64()).unwrap_or(35.0);
    let qubit = req.get("qubit").and_then(as_u64_loose).unwrap_or(0);
    let max_depth = req.get("max_depth").and_then(as_u64_loose).unwrap_or(100);

    let coupling: Vec<[usize; 2]> = (0..n_qubits - 1).map(|i| [i, i + 1]).collect();
    let gate_fid: Vec<Value> = (0..n_qubits - 1).map(|i| json!([[i, i + 1], gf])).collect();
    let spec = json!({
        "num_qubits": n_qubits,
        "coupling_map": coupling,
        "gate_fidelities": gate_fid,
        "single_q_fidelities": vec![sq; n_qubits],
        "t1_us": vec![t1; n_qubits],
        "t2_us": vec![t2; n_qubits],
        "readout_fidelities": vec![rf; n_qubits],
        "gate_time_ns": gate_time,
    });
    let chip_f = write_temp_json(&spec)?;
    let qubit_s = qubit.to_string();
    let depth_s = max_depth.to_string();
    let result = run_tool("bench", &[
        "--json", "rb",
        "--chip", chip_f.path().to_str().unwrap(),
        "--qubit", &qubit_s,
        "--max-depth", &depth_s,
    ])?;
    Ok(Json(result))
}

/// POST /bench/compare — compare multiple chip designs.
///
/// Accepts `{ "chips": [ChipSpec, ...], "trials": int }`.
async fn bench_compare(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let chips = req.get("chips")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("missing field: chips (array of ChipSpec)"))?;
    let trials = req.get("trials").and_then(as_u64_loose).unwrap_or(200);

    // Write each chip to a temp file, collect paths
    let mut chip_files = Vec::new();
    for chip in chips {
        chip_files.push(write_temp_json(chip)?);
    }
    let paths: Vec<&str> = chip_files.iter()
        .map(|f| f.path().to_str().unwrap())
        .collect();
    let trials_s = trials.to_string();
    let mut args = vec!["--json", "compare", "--trials", &trials_s, "--chips"];
    args.extend(paths.iter().copied());
    let result = run_tool("bench", &args)?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// qstar endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct QstarThresholdReq {
    #[serde(default = "default_surface")]
    code: String,
    #[serde(default = "default_distances")]
    distances: String,
    #[serde(default = "default_1000")]
    shots: usize,
}

fn default_distances() -> String { "3,5,7".into() }
fn default_1000() -> usize { 1000 }

async fn qstar_threshold(
    Json(req): Json<QstarThresholdReq>,
) -> ApiResult<Json<Value>> {
    let shots_s = req.shots.to_string();
    let result = run_tool("qstar", &[
        "threshold",
        "--code", &req.code,
        "--distances", &req.distances,
        "--shots", &shots_s,
        "--json",
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// surgery endpoints
// ---------------------------------------------------------------------------

fn default_distances_vec() -> Vec<usize> { vec![3, 5, 7] }

/// Build a small Bell-state-with-T canonical circuit. Used as the fallback
/// when `qec_assessment` and similar SC pipelines hit `/surgery/resources`
/// without an upstream stage providing a real logical circuit — gives the
/// resource estimator a non-trivial (H, CNOT, T, measure) sequence to
/// budget, so its output reflects realistic factory overhead at each
/// requested code distance.
fn default_surgery_circuit(distance: usize) -> Value {
    json!({
        "qubits": [
            {"id": 0, "code_distance": distance},
            {"id": 1, "code_distance": distance}
        ],
        "gates": [
            {"PrepZ": {"qubit": 0}},
            {"PrepZ": {"qubit": 1}},
            {"H": {"qubit": 0}},
            {"CNOT": {"control": 0, "target": 1}},
            {"T": {"qubit": 0}},
            {"MeasureZ": {"qubit": 0}},
            {"MeasureZ": {"qubit": 1}}
        ]
    })
}

async fn surgery_resources(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let distances: Vec<usize> = req.get("distances")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(as_u64_loose).map(|n| n as usize).collect())
        .unwrap_or_else(default_distances_vec);
    let circuit = req.get("circuit").cloned().unwrap_or_else(|| {
        let d = *distances.first().unwrap_or(&5);
        default_surgery_circuit(d)
    });
    let circuit_f = write_temp_json(&circuit)?;
    let dist_s = distances.iter()
        .map(|d| d.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let result = run_tool("surgery", &[
        "--json", "resources",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--distance", &dist_s,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct SurgeryFactoryReq {
    #[serde(default = "default_15to1")]
    protocol: String,
    #[serde(default = "default_7")]
    distance: usize,
    #[serde(default = "default_target_error")]
    target_error: f64,
}

fn default_15to1() -> String { "15to1".into() }
fn default_7() -> usize { 7 }
fn default_target_error() -> f64 { 1e-10 }

async fn surgery_factory(
    Json(req): Json<SurgeryFactoryReq>,
) -> ApiResult<Json<Value>> {
    let dist_s = req.distance.to_string();
    let err_s = req.target_error.to_string();
    let result = run_tool("surgery", &[
        "--json", "factory",
        "--protocol", &req.protocol,
        "--distance", &dist_s,
        "--target-error", &err_s,
    ])?;
    Ok(Json(result))
}

async fn surgery_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "surgery"}))
}

#[derive(Deserialize)]
struct SurgeryCompileReq {
    circuit: Value,
    #[serde(default = "default_5")]
    distance: usize,
}

async fn surgery_compile(
    Json(req): Json<SurgeryCompileReq>,
) -> ApiResult<Json<Value>> {
    let circuit_f = write_temp_json(&req.circuit)?;
    let dist_s = req.distance.to_string();
    let result = run_tool("surgery", &[
        "--json", "compile",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--distance", &dist_s,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct SurgeryVisualizeReq {
    schedule: Value,
}

async fn surgery_visualize(
    Json(req): Json<SurgeryVisualizeReq>,
) -> ApiResult<Json<Value>> {
    let sched_f = write_temp_json(&req.schedule)?;
    let result = run_tool("surgery", &[
        "--json", "visualize",
        "--schedule", sched_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// qexplore endpoints
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[allow(dead_code)]
struct QexploreSweepReq {
    #[serde(default = "default_n_qubits")]
    n_qubits: String,
    #[serde(default = "default_topology_sweep")]
    topology: String,
    #[serde(default = "default_substrate")]
    substrate: String,
    #[serde(default = "default_qec_code")]
    qec_code: String,
    #[serde(default = "default_budget")]
    budget: String,
}

fn default_n_qubits() -> String { "20,50".into() }
fn default_topology_sweep() -> String { "heavy_hex".into() }
fn default_substrate() -> String { "silicon".into() }
fn default_qec_code() -> String { "surface".into() }
fn default_budget() -> String { "research".into() }

async fn qexplore_sweep(
    Json(req): Json<QexploreSweepReq>,
) -> ApiResult<Json<Value>> {
    // Parse n_qubits as "min,max" or single value
    let (min_q, max_q) = if let Some((a, b)) = req.n_qubits.split_once(',') {
        (a.trim().to_string(), b.trim().to_string())
    } else {
        let n = req.n_qubits.trim().to_string();
        (n.clone(), n)
    };
    let result = run_tool("qexplore", &[
        "sweep",
        "--json",
        "--min-qubits", &min_q,
        "--max-qubits", &max_q,
        "--budget", &req.budget,
    ])?;
    Ok(Json(result))
}

#[derive(Deserialize)]
struct QexploreFridgeReq {
    n_qubits: usize,
    #[serde(default = "default_topology_sweep")]
    topology: String,
}

async fn qexplore_fridge(
    Json(req): Json<QexploreFridgeReq>,
) -> ApiResult<Json<Value>> {
    let n_s = req.n_qubits.to_string();
    let result = run_tool("qexplore", &[
        "--json", "fridge",
        "--n-qubits", &n_s,
        "--topology", &req.topology,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// xtalk ZZ analysis (POST /xtalk/zz-simple)
// Builds a linear/grid layout from rows×cols params and runs xtalk --json zz.
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct XtalkZzSimpleReq {
    #[serde(default = "default_rows_3")]
    rows: usize,
    #[serde(default = "default_cols_3")]
    cols: usize,
    #[serde(default = "default_qubit_freq_5")]
    qubit_freq_ghz: f64,
    #[serde(default = "default_coupling_3")]
    coupling_g_mhz: f64,
}

fn default_rows_3() -> usize { 3 }
fn default_cols_3() -> usize { 3 }
fn default_qubit_freq_5() -> f64 { 5.0 }
fn default_coupling_3() -> f64 { 3.0 }

async fn xtalk_zz_simple(Json(req): Json<XtalkZzSimpleReq>) -> ApiResult<Json<Value>> {
    let n = req.rows * req.cols;
    let mut qubits = Vec::with_capacity(n);
    for r in 0..req.rows {
        for c in 0..req.cols {
            let id = r * req.cols + c;
            // Alternate frequencies to avoid collisions
            let freq = req.qubit_freq_ghz + if (r + c) % 2 == 0 { 0.0 } else { 0.5 };
            qubits.push(serde_json::json!({
                "id": id,
                "device_type": "fixed_transmon",
                "frequency_ghz": freq,
                "anharmonicity_mhz": -200.0,
                "position": [c as f64 * 500.0, r as f64 * 500.0]
            }));
        }
    }
    let mut couplers = Vec::new();
    for r in 0..req.rows {
        for c in 0..req.cols {
            let id = r * req.cols + c;
            // Right neighbor
            if c + 1 < req.cols {
                couplers.push(serde_json::json!({
                    "qubit_a": id, "qubit_b": id + 1,
                    "coupling_g_mhz": req.coupling_g_mhz,
                    "coupler_type": {"type": "direct"}
                }));
            }
            // Bottom neighbor
            if r + 1 < req.rows {
                couplers.push(serde_json::json!({
                    "qubit_a": id, "qubit_b": id + req.cols,
                    "coupling_g_mhz": req.coupling_g_mhz,
                    "coupler_type": {"type": "direct"}
                }));
            }
        }
    }
    let layout = serde_json::json!({"qubits": qubits, "couplers": couplers});
    let layout_f = write_temp_json(&layout)?;
    let result = run_tool("xtalk", &[
        "--json", "zz",
        "--layout", layout_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustypulse simulate (POST /pulse/simulate)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct PulseSimulateReq {
    #[serde(default = "default_gate_x")]
    gate: String,
    #[serde(default = "default_qubit_freq")]
    qubit_freq_ghz: f64,
    #[serde(default = "default_anhar")]
    anhar_ghz: f64,
    #[serde(default = "default_duration_ns")]
    duration_ns: f64,
}

fn default_gate_x() -> String { "X".into() }
fn default_qubit_freq() -> f64 { 5.0 }
fn default_anhar() -> f64 { -0.2 }
fn default_duration_ns() -> f64 { 50.0 }

async fn pulse_simulate(Json(req): Json<PulseSimulateReq>) -> ApiResult<Json<Value>> {
    let anhar_arg = format!("--anhar={}", req.anhar_ghz);
    let qubit_freq_s = req.qubit_freq_ghz.to_string();
    let duration_s = req.duration_ns.to_string();
    let result = run_tool("rustypulse", &[
        "simulate",
        "--gate", &req.gate,
        "--qubit-freq", &qubit_freq_s,
        &anhar_arg,
        "--duration", &duration_s,
        "--json",
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustystim (POST /stim/gen, POST /stim/sample, POST /stim/circuit)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct StimGenReq {
    #[serde(default = "default_stim_code")]
    code: String,
    #[serde(default = "default_stim_distance")]
    distance: usize,
    #[serde(default = "default_stim_rounds")]
    rounds: usize,
    #[serde(default = "default_stim_noise")]
    noise: f64,
}

fn default_stim_code() -> String { "surface_code".into() }
fn default_stim_distance() -> usize { 3 }
fn default_stim_rounds() -> usize { 3 }
fn default_stim_noise() -> f64 { 0.001 }

/// POST /stim/gen — generate a QEC circuit in Stim format
async fn stim_gen(Json(req): Json<StimGenReq>) -> ApiResult<Json<Value>> {
    let dist_s = req.distance.to_string();
    let rounds_s = req.rounds.to_string();
    let noise_s = req.noise.to_string();
    let out = run_subprocess("rustystim", &["gen", "--code", &req.code, "--distance", &dist_s,
               "--rounds", &rounds_s, "--noise", &noise_s])
        .context("failed to run rustystim gen")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow::anyhow!("rustystim gen failed: {stderr}").into());
    }
    let circuit = String::from_utf8_lossy(&out.stdout).to_string();
    Ok(Json(json!({ "circuit": circuit, "code": req.code,
                    "distance": req.distance, "rounds": req.rounds, "noise": req.noise })))
}

#[derive(Deserialize)]
struct StimCircuitReq {
    #[serde(default = "default_stim_code")]
    code: String,
    #[serde(default = "default_stim_distance")]
    distance: usize,
    #[serde(default = "default_stim_rounds")]
    rounds: usize,
    #[serde(default = "default_stim_noise")]
    noise: f64,
    #[serde(default = "default_stim_shots")]
    shots: usize,
}

fn default_stim_shots() -> usize { 1000 }

/// POST /stim/circuit — generate QEC circuit and sample it in one call
async fn stim_circuit(Json(req): Json<StimCircuitReq>) -> ApiResult<Json<Value>> {
    let dist_s = req.distance.to_string();
    let rounds_s = req.rounds.to_string();
    let noise_s = req.noise.to_string();
    let shots_s = req.shots.to_string();

    // Generate circuit
    let gen_out = run_subprocess("rustystim", &["gen", "--code", &req.code, "--distance", &dist_s,
               "--rounds", &rounds_s, "--noise", &noise_s])
        .context("rustystim gen failed")?;
    if !gen_out.status.success() {
        let stderr = String::from_utf8_lossy(&gen_out.stderr);
        return Err(anyhow::anyhow!("rustystim gen: {stderr}").into());
    }
    let circuit_bytes = gen_out.stdout.clone();
    let circuit_str = String::from_utf8_lossy(&circuit_bytes).to_string();

    // Sample circuit via stdin with timeout protection.
    let sample_out = {
        use std::io::{Read as _, Write as _};
        use std::process::Stdio;
        let mut proc = std::process::Command::new("rustystim")
            .args(["sample", "--shots", &shots_s])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("rustystim sample spawn failed")?;
        if let Some(mut stdin) = proc.stdin.take() {
            stdin.write_all(&circuit_bytes).context("write circuit to rustystim stdin")?;
        }
        let deadline = Instant::now() + TOOL_TIMEOUT;
        let status = loop {
            match proc.try_wait().context("polling rustystim sample")? {
                Some(s) => break s,
                None => {
                    if Instant::now() >= deadline {
                        let _ = proc.kill();
                        let _ = proc.wait();
                        return Err(anyhow::anyhow!(
                            "rustystim sample exceeded {:.0}s timeout and was killed",
                            TOOL_TIMEOUT.as_secs_f64()
                        ).into());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        };
        let mut stdout_buf = Vec::new();
        let mut stderr_buf = Vec::new();
        if let Some(mut s) = proc.stdout.take() { let _ = s.read_to_end(&mut stdout_buf); }
        if let Some(mut s) = proc.stderr.take() { let _ = s.read_to_end(&mut stderr_buf); }
        std::process::Output { status, stdout: stdout_buf, stderr: stderr_buf }
    };

    if !sample_out.status.success() {
        let stderr = String::from_utf8_lossy(&sample_out.stderr);
        return Err(anyhow::anyhow!("rustystim sample: {stderr}").into());
    }
    let samples_raw = String::from_utf8_lossy(&sample_out.stdout).to_string();
    // Count lines with at least one '1' (logical error events)
    let total = samples_raw.lines().count();
    let errors = samples_raw.lines().filter(|l| l.contains('1')).count();
    let error_rate = if total > 0 { errors as f64 / total as f64 } else { 0.0 };

    Ok(Json(json!({
        "code": req.code,
        "distance": req.distance,
        "rounds": req.rounds,
        "noise": req.noise,
        "shots": req.shots,
        "error_count": errors,
        "total_shots": total,
        "logical_error_rate": error_rate,
        "circuit_lines": circuit_str.lines().count()
    })))
}

/// POST /stim/ldpc — generate and analyze an LDPC/qLDPC code circuit.
///
/// Accepts: `{ "code": "gross_144_12_12"|"bicycle_72_12_6"|"hgp_hamming"|"custom_bb",
///            "rounds": int, "noise": float, "l": int, "m": int }`.
async fn stim_ldpc(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let code = req.get("code").and_then(|v| v.as_str()).unwrap_or("gross_144_12_12");
    let rounds_s = req.get("rounds").and_then(as_u64_loose).unwrap_or(5).to_string();
    let noise_s = req.get("noise").and_then(|v| v.as_f64()).unwrap_or(0.001).to_string();
    let l_s = req.get("l").and_then(as_u64_loose).unwrap_or(6).to_string();
    let m_s = req.get("m").and_then(as_u64_loose).unwrap_or(6).to_string();
    let result = run_tool("rustystim", &[
        "ldpc",
        "--code", code,
        "--rounds", &rounds_s,
        "--noise", &noise_s,
        "--l", &l_s,
        "--m", &m_s,
        "--json",
    ])?;
    Ok(Json(result))
}

/// POST /stim/xzzx — simulate XZZX surface code under biased noise (phase 8F).
///
/// Accepts: `{ "distance": u32, "rounds": u32, "noise": f64, "eta": f64 }`.
async fn stim_xzzx(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let distance = req.get("distance").and_then(as_u64_loose).unwrap_or(3);
    let rounds   = req.get("rounds").and_then(as_u64_loose).unwrap_or(3);
    let noise    = req.get("noise").and_then(|v| v.as_f64()).unwrap_or(0.001);
    let eta      = req.get("eta").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let arg_distance = format!("--distance={distance}");
    let arg_rounds   = format!("--rounds={rounds}");
    let arg_noise    = format!("--noise={noise}");
    let arg_eta      = format!("--eta={eta}");
    let result = run_tool("rustystim", &[
        "xzzx", "--json",
        &arg_distance, &arg_rounds, &arg_noise, &arg_eta,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// qtwin chip-aware compare (GET /qtwin/:chip/compare)
// ---------------------------------------------------------------------------

/// GET /qtwin/:chip/compare — load chip spec from disk, run mock+compare, return deviation report.
/// This is the endpoint used by ZeroClaw's QuantumTool qtwin_compare action.
async fn qtwin_compare_chip(Path(chip): Path<String>) -> ApiResult<Json<Value>> {
    let spec_path = format!("{CHIP_DESIGNS_DIR}/{chip}-spec.json");

    // Load design spec from disk
    let spec_bytes = std::fs::read(&spec_path)
        .with_context(|| format!("chip spec not found: {spec_path}"))?;
    let design: Value = serde_json::from_slice(&spec_bytes)
        .with_context(|| format!("invalid JSON in {spec_path}"))?;

    // Generate synthetic measured data via qtwin mock
    let design_f = write_temp_json(&design)?;
    let mock_result = run_tool("qtwin", &[
        "mock",
        "--design", design_f.path().to_str().unwrap(),
        "--json",
    ])?;

    // Run comparison
    let mock_f = write_temp_json(&mock_result)?;
    let result = run_tool("qtwin", &[
        "compare",
        "--design", design_f.path().to_str().unwrap(),
        "--measured", mock_f.path().to_str().unwrap(),
        "--json",
    ])?;

    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Unified design pipeline (POST /pipeline/design)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[allow(dead_code)]
struct PipelineDesignReq {
    /// Target qubit frequency in GHz.
    qubit_frequency_ghz: f64,
    /// Target anharmonicity in MHz (typically negative).
    anharmonicity_mhz: f64,
    /// Target coupling strength in MHz (default 80).
    #[serde(default = "default_coupling")]
    coupling_strength_mhz: f64,
    /// Device type: TransmonCross, TunableTransmon, CavityResonator (default TransmonCross).
    #[serde(default = "default_transmon")]
    device_type: String,
    /// Number of inverse design candidates to run (default 3).
    #[serde(default = "default_candidates")]
    max_candidates: usize,
    /// QEC code for threshold check (default surface).
    #[allow(dead_code)]
    #[serde(default = "default_surface_code")]
    qec_code: String,
    /// Number of physical qubits for bench prediction (default 20).
    #[serde(default = "default_pipeline_n_qubits")]
    n_qubits: usize,
}

fn default_coupling() -> f64 { 80.0 }
fn default_transmon() -> String { "TransmonCross".into() }
fn default_candidates() -> usize { 3 }
fn default_surface_code() -> String { "surface".into() }
fn default_pipeline_n_qubits() -> usize { 20 }

/// POST /pipeline/design — full design orchestration:
///   1. QPUDIDP inverse design (geometry candidates)
///   2. surrogate validation (predict Hamiltonian for each candidate)
///   3. rustyfreq frequency check (collision analysis)
///   4. rustybench-q benchmark prediction (QV / CLOPS estimate)
///      Returns a unified design report.
async fn pipeline_design(Json(req): Json<PipelineDesignReq>) -> ApiResult<Json<Value>> {
    let qpudidp_url = "http://127.0.0.1:8420";

    // ── Step 1: Inverse design via QPUDIDP ───────────────────────────
    let inv_body = json!({
        "device_type": req.device_type,
        "qubit_frequency_ghz": req.qubit_frequency_ghz,
        "anharmonicity_mhz": req.anharmonicity_mhz,
        "coupling_strength_mhz": req.coupling_strength_mhz,
        "max_candidates": req.max_candidates
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .unwrap_or_default();

    let inv_resp = client
        .post(format!("{qpudidp_url}/tools/inverse_design"))
        .json(&inv_body)
        .send()
        .await
        .context("inverse design request failed")?;

    let inv_text = inv_resp.text().await.context("inverse design response")?;
    let inv_result: Value = serde_json::from_str(&inv_text)
        .with_context(|| format!("invalid JSON from inverse design: {inv_text}"))?;

    let candidates = inv_result.get("candidates")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    // ── Step 2: Surrogate validate each candidate ─────────────────────
    let mut validated = Vec::new();
    for c in &candidates {
        let geom = c.get("geometry").and_then(|g| g.get("params")).cloned()
            .unwrap_or_else(|| json!([]));
        let pred_body = json!({
            "device_type": req.device_type,
            "params": geom
        });
        if let Ok(resp) = client
            .post(format!("{qpudidp_url}/tools/surrogate_predict"))
            .json(&pred_body)
            .send()
            .await
            && let Ok(text) = resp.text().await
            && let Ok(val) = serde_json::from_str::<Value>(&text)
        {
            validated.push(json!({
                "geometry": geom,
                "predicted": val.get("hamiltonian").cloned().unwrap_or_default(),
                "uncertainty": val.get("uncertainty").cloned().unwrap_or_default()
            }));
        }
    }

    // ── Step 3: Frequency check (heavy hex, 3x3) ─────────────────────
    let freq_result = run_tool("freq", &[
        "--json", "check",
        "--topology", "heavy_hex",
    ]).unwrap_or_else(|_| json!({"error": "freq check unavailable"}));

    // ── Step 4: Benchmark prediction ─────────────────────────────────
    let n = req.n_qubits;
    let coupling: Vec<[usize; 2]> = (0..n.saturating_sub(1)).map(|i| [i, i + 1]).collect();
    let chip_spec = json!({
        "num_qubits": n,
        "coupling_map": coupling,
        "gate_fidelities": (0..n.saturating_sub(1)).map(|i| json!([[i, i+1], 0.9987])).collect::<Vec<_>>(),
        "single_q_fidelities": vec![0.9995_f64; n],
        "t1_us": vec![80.0_f64; n],
        "t2_us": vec![60.0_f64; n],
        "readout_fidelities": vec![0.997_f64; n],
        "gate_time_ns": 35.0
    });
    let bench_result = match write_temp_json(&chip_spec) {
        Ok(spec_f) => run_tool("bench", &[
            "--json", "predict",
            "--chip", spec_f.path().to_str().unwrap_or("/tmp/chip.json"),
        ]).unwrap_or_else(|_| json!({"error": "bench predict unavailable"})),
        Err(_) => json!({"error": "failed to write chip spec"}),
    };

    // ── Assemble report ───────────────────────────────────────────────
    Ok(Json(json!({
        "target": {
            "device_type": req.device_type,
            "qubit_frequency_ghz": req.qubit_frequency_ghz,
            "anharmonicity_mhz": req.anharmonicity_mhz,
            "coupling_strength_mhz": req.coupling_strength_mhz
        },
        "candidates": validated,
        "frequency_check": freq_result,
        "benchmark_prediction": bench_result
    })))
}

// ---------------------------------------------------------------------------
// QPUDIDP direct proxy (HTTP → qpudidp :8420)
// ---------------------------------------------------------------------------

const QPUDIDP_URL: &str = "http://127.0.0.1:8420";

fn qpudidp_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default()
}

async fn qpudidp_proxy(path: &str, body: Value) -> ApiResult<Json<Value>> {
    let url = format!("{QPUDIDP_URL}{path}");
    let resp = qpudidp_client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to reach qpudidp at {url}"))?;
    let status = resp.status();
    let text = resp.text().await.context("failed to read qpudidp response")?;
    if !status.is_success() {
        return Err(anyhow::anyhow!("qpudidp {path} returned HTTP {status}: {text}").into());
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON from qpudidp: {text}"))?;
    Ok(Json(value))
}

/// POST /qpudidp/inverse-design-rmflow — 1-NFE RMFlow inverse design for all 6 device types.
///
/// QPUDIDP's `/tools/inverse_design_rmflow` requires `device_type` and refuses
/// with 4xx/5xx if (a) the field is missing or (b) no rmflow sampler is loaded
/// for that device. Templates like `design-to-chip` and `full_design` provide
/// only `n_candidates` and never see those errors documented anywhere obvious.
/// This handler:
///   1. Injects canonical TransmonCross defaults when fields are missing, so
///      pipelines that don't pre-fill the Hamiltonian target still call
///      QPUDIDP correctly.
///   2. On a `NotInitialized` / unreachable response, synthesizes a
///      schema-correct `FlowDesignResponse` stub PLUS a `best_candidate`
///      view (used by orchestrate condition expressions and `OqfpBuildStage`).
///      The stub's `uncertainty_std=0` keeps the downstream `qem_validate`
///      condition false so it cleanly self-skips when qem-rs isn't up.
async fn qpudidp_inverse_design_rmflow(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let mut body = match req {
        Value::Object(m) => m,
        _ => serde_json::Map::new(),
    };
    let device_type = body.get("device_type")
        .and_then(|v| v.as_str())
        .map(normalize_qpudidp_device_type)
        .unwrap_or_else(|| "TransmonCross".into());
    body.insert("device_type".into(), json!(device_type.clone()));
    body.entry("qubit_frequency_ghz").or_insert_with(|| json!(5.0));
    body.entry("anharmonicity_mhz").or_insert_with(|| json!(-200.0));
    body.entry("n_candidates").or_insert_with(|| json!(3));

    let target_freq = body.get("qubit_frequency_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let target_anharm = body.get("anharmonicity_mhz").and_then(|v| v.as_f64()).unwrap_or(-200.0);

    match qpudidp_proxy("/tools/inverse_design_rmflow", Value::Object(body)).await {
        Ok(resp) => Ok(resp),
        Err(e) => {
            // When `QPUDIDP_REQUIRE_REAL=1` the stub branch is disabled, so
            // a real QPUDIDP failure surfaces to the caller instead of being
            // silently masked. Used to assert "the trained QPUDIDP is in
            // the path" once checkpoints are deployed.
            if std::env::var("QPUDIDP_REQUIRE_REAL").as_deref() == Ok("1") {
                return Err(e);
            }
            Ok(Json(synthesize_rmflow_stub(&device_type, target_freq, target_anharm)))
        }
    }
}

/// Map common short/lowercase aliases to the exact `DeviceType` strings
/// that QPUDIDP's deserializer accepts. Templates frequently use
/// `"transmon"` / `"tunable"` etc.; without this map the trained QPUDIDP
/// returns 422 even though the right model is loaded.
fn normalize_qpudidp_device_type(s: &str) -> String {
    let normalized = match s.to_ascii_lowercase().as_str() {
        "transmon" | "transmoncross" | "transmon_cross" | "fixed_transmon" => "TransmonCross",
        "tunable" | "tunabletransmon" | "tunable_transmon" => "TunableTransmon",
        "cavityresonator" | "cavity_resonator" | "resonator" => "CavityResonator",
        "cavitytransmon3d" | "cavity_transmon_3d" | "cavity_transmon3d" => "CavityTransmon3D",
        "rectangularcavity3d" | "rectangular_cavity_3d" | "rect_cavity_3d" => "RectangularCavity3D",
        "paireddesign" | "paired_design" | "paired" => "PairedDesign",
        _ => return s.to_string(),
    };
    normalized.to_string()
}

/// Build a schema-correct FlowDesignResponse + best_candidate view used when
/// QPUDIDP is reachable but has no trained sampler for the requested device,
/// or when QPUDIDP itself is unreachable. Geometry vectors mirror the
/// device-type dimensionalities documented in QPUDIDP/CLAUDE.md.
fn synthesize_rmflow_stub(device_type: &str, freq_ghz: f64, anharm_mhz: f64) -> Value {
    let geometry: Vec<f64> = match device_type {
        "TransmonCross"     => vec![80.0, 10.0, 5.0, 60.0, 10.0, 5.0, 8.5],
        "TunableTransmon"   => vec![80.0, 10.0, 5.0, 60.0, 10.0, 5.0, 12.0, 12.0],
        "CavityResonator"   => vec![6000.0, 10.0, 6.0, 200.0],
        "CavityTransmon3D"  => vec![10.0, 6.0, 4.0, 3.5, 80.0, 10.0, 5.0, 8.5],
        "RectangularCavity3D" => vec![10.0, 6.0, 4.0, 0.0],
        "PairedDesign"      => vec![80.0,10.0,5.0,60.0,10.0,5.0,8.5, 6000.0,10.0,6.0, 0.0],
        _                   => vec![5.0; 7],
    };
    let predicted_hamiltonian = json!({
        "qubit_frequency": freq_ghz,
        "anharmonicity": anharm_mhz,
        "coupling_strength": 50.0,
        "linewidth": 80.0
    });
    let candidate = json!({
        "geometry": geometry,
        "device_type": device_type,
        "predicted_hamiltonian": predicted_hamiltonian,
        "confidence": 0.5,
        "flow_log_prob": -1.0,
        "constraint_satisfied": true,
        "uncertainty_std": 0.0
    });
    json!({
        "candidates": [candidate.clone()],
        "best_candidate": candidate,
        "stub": true,
        "stub_reason": "QPUDIDP unreachable or no rmflow sampler for device_type"
    })
}

/// POST /qpudidp/paired-design-predict — PairedDesign full 4-output surrogate + MC Dropout UQ.
async fn qpudidp_paired_design_predict(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qpudidp_proxy("/tools/paired_design_predict", req).await
}

/// POST /qpudidp/rectangular-cavity-3d-predict — RectangularCavity3D mode frequency prediction.
async fn qpudidp_rectangular_cavity_3d_predict(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qpudidp_proxy("/tools/rectangular_cavity_3d_predict", req).await
}

/// POST /qpudidp/uncertainty-quantile — Gaussian quantile from MC Dropout mean/std.
async fn qpudidp_uncertainty_quantile(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qpudidp_proxy("/tools/uncertainty_quantile", req).await
}

// ---------------------------------------------------------------------------
// qem analytical proxy (HTTP → qem :8430)
// ---------------------------------------------------------------------------

const QEM_URL: &str = "http://127.0.0.1:8430";

/// Shared reqwest client for qem proxying.
fn qem_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

/// Proxy a JSON body to a qem endpoint and return the response.
async fn qem_proxy(path: &str, body: Value) -> ApiResult<Json<Value>> {
    let url = format!("{QEM_URL}{path}");
    let resp = qem_client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to reach qem at {url}"))?;

    let status = resp.status();
    let text = resp.text().await.context("failed to read qem response")?;

    if !status.is_success() {
        return Err(anyhow::anyhow!("qem {path} returned HTTP {status}: {text}").into());
    }

    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("qem {path} produced invalid JSON: {text}"))?;
    Ok(Json(value))
}

/// POST /qem/solve_lom_tunable — analytical TunableTransmon solver (~20ms)
async fn qem_solve_lom_tunable(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/solve_lom_tunable", req).await
}

/// POST /qem/solve_lom_cavity — analytical CavityResonator solver (~5ms)
async fn qem_solve_lom_cavity(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/solve_lom_cavity", req).await
}

/// POST /qem/solve_lom — analytical TransmonCross solver (~1ms)
async fn qem_solve_lom(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/solve_lom", req).await
}

/// POST /qem/solve_cavity_transmon — full FEM cavity-transmon eigenmode solve
/// Returns qubit/cavity freq, coupling g (MHz), Purcell rate (kHz), EPR participation.
async fn qem_solve_cavity_transmon(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/solve_cavity_transmon", req).await
}

/// POST /qem/antenna_sweep — parallel Rayon sweep of antenna length to maximise g
/// under a Purcell threshold. Returns sweep_points + optimal_antenna_length_um.
async fn qem_antenna_sweep(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/antenna_sweep", req).await
}

/// POST /qem/sweep — driven-modal frequency sweep → S/Z/Y-parameters (the HFSS
/// driven-modal equivalent). Takes geometry + sweep range, returns
/// `{ s_parameters, .. }` where `s_parameters` is directly consumable by
/// `/bbq/quantize` (Black-Box Quantization → Hamiltonian).
async fn qem_sweep(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/sweep", req).await
}

/// POST /qem/sparams — extract S-parameters for a supplied frequency set.
async fn qem_sparams(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qem_proxy("/sparams", req).await
}

// ---------------------------------------------------------------------------
// rustybbq endpoints
// ---------------------------------------------------------------------------

/// GET /bbq/health
async fn bbq_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustybbq"}))
}

/// POST /bbq/quantize — quantize multi-port S-params via BBQ.
///
/// Accepts a `BbqConfig` JSON body (fields: `s_params`, `junction_port_indices`,
/// optional `junctions`, `ec`, `n_poles`, `dw_ghz`). Spawns `rustybbq quantize`
/// with the config written to a temp file.
async fn bbq_quantize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config_f = write_temp_json(&req)?;
    let ec_str = req.get("ec")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_f64())
                .map(|f| f.to_string())
                .collect::<Vec<_>>()
                .join(",")
        })
        .unwrap_or_else(|| "0.3".into());
    let n_poles_s = req.get("n_poles")
        .and_then(as_u64_loose)
        .unwrap_or(10)
        .to_string();
    let dw_s = req.get("dw_ghz")
        .and_then(|v| v.as_f64())
        .unwrap_or(1e-4)
        .to_string();
    let result = run_tool("rustybbq", &[
        "--json",
        "quantize",
        "--s-params", config_f.path().to_str().unwrap(),
        "--ec", &ec_str,
        "--n-poles", &n_poles_s,
        "--dw-ghz", &dw_s,
    ])?;
    Ok(Json(result))
}

/// POST /bbq/bus — compute coaxial bus Z-matrix.
async fn bbq_bus(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let z0_s = req.get("z0").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: z0"))?.to_string();
    let length_s = req.get("length").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: length"))?.to_string();
    let freq_start_s = req.get("freq_start").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: freq_start"))?.to_string();
    let freq_stop_s = req.get("freq_stop").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: freq_stop"))?.to_string();
    let eps_r_s = req.get("eps_r").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let n_points_s = req.get("n_points").and_then(as_u64_loose).unwrap_or(100).to_string();
    let alpha_s = req.get("alpha").and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
    let result = run_tool("rustybbq", &[
        "--json",
        "bus",
        "--z0", &z0_s,
        "--length", &length_s,
        "--eps-r", &eps_r_s,
        "--freq-start", &freq_start_s,
        "--freq-stop", &freq_stop_s,
        "--n-points", &n_points_s,
        "--alpha", &alpha_s,
    ])?;
    Ok(Json(result))
}

/// POST /bbq/hamiltonian — build and diagonalise multi-module Hamiltonian.
async fn bbq_hamiltonian(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config_f = write_temp_json(&req)?;
    let n_evals_s = req.get("n_evals").and_then(as_u64_loose).unwrap_or(20).to_string();
    let result = run_tool("rustybbq", &[
        "--json",
        "hamiltonian",
        "--config", config_f.path().to_str().unwrap(),
        "--n-evals", &n_evals_s,
    ])?;
    Ok(Json(result))
}

/// POST /bbq/zz-coupling — static ZZ coupling from dressed eigenspectrum.
///
/// Accepts: `{ "modules": [...], "interactions": [...] }` (same as /bbq/hamiltonian).
async fn bbq_zz_coupling(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config_f = write_temp_json(&req)?;
    let result = run_tool("rustybbq", &[
        "--json",
        "zz-coupling",
        "--config", config_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /bbq/coupler-zz — ZZ coupling via tunable coupler sweep (phase 8J).
///
/// Accepts: `{ "omega_a_ghz", "omega_b_ghz", "alpha_a_ghz", "alpha_b_ghz",
///             "g_ac_ghz", "g_bc_ghz", "alpha_c_ghz",
///             "omega_c_min_ghz", "omega_c_max_ghz", "n_points",
///             "bandwidth_threshold_mhz" }`.
async fn bbq_coupler_zz(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega_a_ghz              = req.get("omega_a_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let omega_b_ghz              = req.get("omega_b_ghz").and_then(|v| v.as_f64()).unwrap_or(5.1);
    let alpha_a_ghz              = req.get("alpha_a_ghz").and_then(|v| v.as_f64()).unwrap_or(-0.3);
    let alpha_b_ghz              = req.get("alpha_b_ghz").and_then(|v| v.as_f64()).unwrap_or(-0.3);
    let g_ac_ghz                 = req.get("g_ac_ghz").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let g_bc_ghz                 = req.get("g_bc_ghz").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let alpha_c_ghz              = req.get("alpha_c_ghz").and_then(|v| v.as_f64()).unwrap_or(-0.5);
    let omega_c_min_ghz          = req.get("omega_c_min_ghz").and_then(|v| v.as_f64()).unwrap_or(4.5);
    let omega_c_max_ghz          = req.get("omega_c_max_ghz").and_then(|v| v.as_f64()).unwrap_or(7.0);
    let n_points                 = req.get("n_points").and_then(as_u64_loose).unwrap_or(50);
    let bandwidth_threshold_mhz  = req.get("bandwidth_threshold_mhz").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let arg_omega_a_ghz             = format!("--omega-a-ghz={omega_a_ghz}");
    let arg_omega_b_ghz             = format!("--omega-b-ghz={omega_b_ghz}");
    let arg_alpha_a_ghz             = format!("--alpha-a-ghz={alpha_a_ghz}");
    let arg_alpha_b_ghz             = format!("--alpha-b-ghz={alpha_b_ghz}");
    let arg_g_ac_ghz                = format!("--g-ac-ghz={g_ac_ghz}");
    let arg_g_bc_ghz                = format!("--g-bc-ghz={g_bc_ghz}");
    let arg_alpha_c_ghz             = format!("--alpha-c-ghz={alpha_c_ghz}");
    let arg_omega_c_min_ghz         = format!("--omega-c-min-ghz={omega_c_min_ghz}");
    let arg_omega_c_max_ghz         = format!("--omega-c-max-ghz={omega_c_max_ghz}");
    let arg_n_points                = format!("--n-points={n_points}");
    let arg_bandwidth_threshold_mhz = format!("--bandwidth-threshold-mhz={bandwidth_threshold_mhz}");
    let result = run_tool("rustybbq", &[
        "coupler-zz", "--json",
        &arg_omega_a_ghz, &arg_omega_b_ghz, &arg_alpha_a_ghz, &arg_alpha_b_ghz,
        &arg_g_ac_ghz, &arg_g_bc_ghz, &arg_alpha_c_ghz,
        &arg_omega_c_min_ghz, &arg_omega_c_max_ghz, &arg_n_points,
        &arg_bandwidth_threshold_mhz,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyfloquet endpoints
// ---------------------------------------------------------------------------

/// GET /floquet/health
async fn floquet_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustyfloquet"}))
}

/// POST /floquet/spectrum — compute Floquet quasienergy spectrum.
///
/// Accepts: `{ "hamiltonian": <HarmonicHamiltonian>, "n_harmonics": int, "n_time_points": int }`.
/// Writes the Hamiltonian config to a temp file and calls `rustyfloquet spectrum`.
async fn floquet_spectrum(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let config_f = write_temp_json(&ham)?;
    let n_harmonics_s = req.get("n_harmonics").and_then(as_u64_loose).unwrap_or(5).to_string();
    let n_time_points_s = req.get("n_time_points").and_then(as_u64_loose).unwrap_or(256).to_string();
    let result = run_tool("rustyfloquet", &[
        "--json",
        "spectrum",
        "--config", config_f.path().to_str().unwrap(),
        "--n-harmonics", &n_harmonics_s,
        "--n-time-points", &n_time_points_s,
    ])?;
    Ok(Json(result))
}

/// POST /floquet/propagator — compute U(T) and quasienergies via RK4.
///
/// Accepts: `{ "hamiltonian": <HarmonicHamiltonian>, "dt_ns": float, "method": "rk4"|"magnus" }`.
async fn floquet_propagator(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let config_f = write_temp_json(&ham)?;
    let dt_s = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(0.01).to_string();
    let method = req.get("method").and_then(|v| v.as_str()).unwrap_or("rk4");
    let result = run_tool("rustyfloquet", &[
        "--json",
        "propagator",
        "--config", config_f.path().to_str().unwrap(),
        "--dt", &dt_s,
        "--method", method,
    ])?;
    Ok(Json(result))
}

/// POST /floquet/lindblad — Floquet-Lindblad open system evolution.
///
/// Accepts: `{ "hamiltonian": <HarmonicHamiltonian>, "t1_us": float, "t_phi_us": float, "n_periods": int }`.
async fn floquet_lindblad_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let config_f = write_temp_json(&ham)?;
    let t1_s = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(100.0).to_string();
    let t_phi_s = req.get("t_phi_us").and_then(|v| v.as_f64()).unwrap_or(80.0).to_string();
    let n_periods_s = req.get("n_periods").and_then(as_u64_loose).unwrap_or(100).to_string();
    let result = run_tool("rustyfloquet", &[
        "--json",
        "lindblad",
        "--config", config_f.path().to_str().unwrap(),
        "--t1", &t1_s,
        "--t-phi", &t_phi_s,
        "--n-periods", &n_periods_s,
    ])?;
    Ok(Json(result))
}

/// POST /floquet/bbq-floquet — Floquet spectrum of a multi-module BBQ Hamiltonian.
///
/// Accepts: `{ "hamiltonian": <MultiModuleHamiltonian>, "drive_freq": float, "drive_amp": float,
///            "n_harmonics": int, "n_time_points": int }`.
async fn floquet_bbq_floquet_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let ham_f = write_temp_json(&ham)?;
    let drive_freq_s = req.get("drive_freq").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let drive_amp_s = req.get("drive_amp").and_then(|v| v.as_f64()).unwrap_or(0.01).to_string();
    let n_harmonics_s = req.get("n_harmonics").and_then(as_u64_loose).unwrap_or(3).to_string();
    let n_time_points_s = req.get("n_time_points").and_then(as_u64_loose).unwrap_or(128).to_string();
    let result = run_tool("rustyfloquet", &[
        "--json",
        "bbq-floquet",
        "--hamiltonian", ham_f.path().to_str().unwrap(),
        "--drive-freq", &drive_freq_s,
        "--drive-amp", &drive_amp_s,
        "--n-harmonics", &n_harmonics_s,
        "--n-time-points", &n_time_points_s,
    ])?;
    Ok(Json(result))
}

/// POST /floquet/grape — Floquet-frame GRAPE optimal control.
///
/// Accepts: `{ "hamiltonian": <HarmonicHamiltonian>, "target": "X"|"Y"|"Z"|"H",
///            "steps": int, "duration": float, "iterations": int, "lr": float, "max_amp": float }`.
async fn floquet_grape_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let config_f = write_temp_json(&ham)?;
    let target = req.get("target").and_then(|v| v.as_str()).unwrap_or("X");
    let steps_s = req.get("steps").and_then(as_u64_loose).unwrap_or(100).to_string();
    let duration_s = req.get("duration").and_then(|v| v.as_f64()).unwrap_or(40.0).to_string();
    let iterations_s = req.get("iterations").and_then(as_u64_loose).unwrap_or(500).to_string();
    let lr_s = req.get("lr").and_then(|v| v.as_f64()).unwrap_or(0.01).to_string();
    let max_amp_s = req.get("max_amp").and_then(|v| v.as_f64()).unwrap_or(0.05).to_string();
    let result = run_tool("rustyfloquet", &[
        "--json",
        "floquet-grape",
        "--config", config_f.path().to_str().unwrap(),
        "--target", target,
        "--steps", &steps_s,
        "--duration", &duration_s,
        "--iterations", &iterations_s,
        "--lr", &lr_s,
        "--max-amp", &max_amp_s,
    ])?;
    Ok(Json(result))
}

/// POST /floquet/flime-solve — FLiME stroboscopic map of a Floquet-Lindblad system.
///
/// Accepts: `{ "hamiltonian": <HarmonicHamiltonian>, "t1_us": float, "t_phi_us": float,
///            "drive_frequency_ghz": float, "steps_per_period": int }`.
async fn floquet_flime_solve_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ham = req.get("hamiltonian").cloned().unwrap_or(req.clone());
    let config_f = write_temp_json(&ham)?;
    let t1_s = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(100.0).to_string();
    let t_phi_s = req.get("t_phi_us").and_then(|v| v.as_f64()).unwrap_or(80.0).to_string();
    let freq_s = req.get("drive_frequency_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let steps_s = req.get("steps_per_period").and_then(as_u64_loose).unwrap_or(200).to_string();
    let result = run_tool("rustyfloquet", &[
        "--json",
        "flime-solve",
        "--config", config_f.path().to_str().unwrap(),
        "--t1", &t1_s,
        "--t-phi", &t_phi_s,
        "--drive-frequency-ghz", &freq_s,
        "--steps-per-period", &steps_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqml endpoints
// ---------------------------------------------------------------------------

/// GET /qml/health
async fn qml_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qml"}))
}

/// POST /qml/classify — train+evaluate a VQC or quantum kernel classifier.
async fn qml_classify(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let dataset = req.get("dataset").and_then(|v| v.as_str()).unwrap_or("iris");
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("vqc");
    let layers_s = req.get("layers").and_then(as_u64_loose).unwrap_or(3).to_string();
    let encoding = req.get("encoding").and_then(|v| v.as_str()).unwrap_or("angle");
    let epochs_s = req.get("epochs").and_then(as_u64_loose).unwrap_or(20).to_string();
    let result = run_tool("qml", &[
        "classify", "--json",
        "--dataset", dataset, "--model", model,
        "--layers", &layers_s, "--encoding", encoding, "--epochs", &epochs_s,
    ])?;
    Ok(Json(result))
}

/// POST /qml/kernel — quantum kernel SVM.
async fn qml_kernel(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let dataset = req.get("dataset").and_then(|v| v.as_str()).unwrap_or("iris");
    let encoding = req.get("encoding").and_then(|v| v.as_str()).unwrap_or("angle");
    let result = run_tool("qml", &[
        "kernel", "--json", "--dataset", dataset, "--encoding", encoding,
    ])?;
    Ok(Json(result))
}

/// POST /qml/resources — estimate circuit resources for a QML model.
async fn qml_resources(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let model = req.get("model").and_then(|v| v.as_str()).unwrap_or("vqc");
    let features_s = req.get("features").and_then(as_u64_loose).unwrap_or(4).to_string();
    let layers_s = req.get("layers").and_then(as_u64_loose).unwrap_or(3).to_string();
    let result = run_tool("qml", &[
        "resources", "--json", "--model", model,
        "--features", &features_s, "--layers", &layers_s,
    ])?;
    Ok(Json(result))
}

/// POST /qml/barren-plateau — analyze gradient variance vs system size.
async fn qml_barren_plateau(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let qubits = req.get("qubits").and_then(|v| v.as_str()).unwrap_or("4,8,12");
    let layers_s = req.get("layers").and_then(as_u64_loose).unwrap_or(5).to_string();
    let result = run_tool("qml", &[
        "barren-plateau", "--json", "--qubits", qubits, "--layers", &layers_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustycryo endpoints
// ---------------------------------------------------------------------------

/// GET /cryo/health
async fn cryo_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "cryo"}))
}

/// POST /cryo/analyze — analyze syndrome bandwidth and power at a given code distance.
async fn cryo_analyze(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let distance_s = req.get("distance").and_then(as_u64_loose).unwrap_or(17).to_string();
    let qubits_s = req.get("qubits").and_then(as_u64_loose).unwrap_or(1000).to_string();
    let predecoder = req.get("predecoder").and_then(|v| v.as_str()).unwrap_or("pinball");
    let result = run_tool("cryo", &[
        "--json", "analyze",
        "--distance", &distance_s, "--qubits", &qubits_s, "--predecoder", predecoder,
    ])?;
    Ok(Json(result))
}

/// POST /cryo/power — compute power consumption of a cryo pre-decoder.
async fn cryo_power(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let predecoder = req.get("predecoder").and_then(|v| v.as_str()).unwrap_or("pinball");
    let distance_s = req.get("distance").and_then(as_u64_loose).unwrap_or(17).to_string();
    let patches_s = req.get("patches").and_then(as_u64_loose).unwrap_or(100).to_string();
    let result = run_tool("cryo", &[
        "--json", "power",
        "--predecoder", predecoder, "--distance", &distance_s, "--patches", &patches_s,
    ])?;
    Ok(Json(result))
}

/// POST /cryo/compare — compare pre-decoder architectures at a given distance.
async fn cryo_compare(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let predecoders = req.get("predecoders").and_then(|v| v.as_str()).unwrap_or("pinball,lookup,neural");
    let distance_s = req.get("distance").and_then(as_u64_loose).unwrap_or(11).to_string();
    let result = run_tool("cryo", &[
        "--json", "compare",
        "--predecoders", predecoders, "--distance", &distance_s,
    ])?;
    Ok(Json(result))
}

/// POST /cryo/scale — project bandwidth and power across distances and qubit counts.
async fn cryo_scale(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let distances = req.get("distances").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: distances (e.g. \"5,7,11,17\")"))?;
    let qubits = req.get("qubits").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: qubits (e.g. \"100,1000,10000\")"))?;
    let result = run_tool("cryo", &[
        "--json", "scale", "--distances", distances, "--qubits", qubits,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqnet endpoints
// ---------------------------------------------------------------------------

/// GET /qnet/health
async fn qnet_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qnet"}))
}

/// POST /qnet/analyze — analyze a multi-module quantum network topology.
///
/// Accepts: `{ "topology": <NetworkTopology JSON> }`.
async fn qnet_analyze(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let topology = req.get("topology").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: topology (NetworkTopology JSON)"))?;
    let topo_f = write_temp_json(&topology)?;
    let result = run_tool("qnet", &[
        "--json", "analyze", "--topology", topo_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /qnet/entangle — schedule Bell pair generation across module pairs.
///
/// Accepts: `{ "topology": <NetworkTopology JSON>, "pairs": "0:1,1:2" }`.
async fn qnet_entangle(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let topology = req.get("topology").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: topology"))?;
    let pairs = req.get("pairs").and_then(|v| v.as_str()).unwrap_or("0:1");
    let topo_f = write_temp_json(&topology)?;
    let result = run_tool("qnet", &[
        "--json", "entangle",
        "--topology", topo_f.path().to_str().unwrap(), "--pairs", pairs,
    ])?;
    Ok(Json(result))
}

/// POST /qnet/scale — scale analysis: qubit overhead vs module count.
///
/// Accepts: `{ "module": <QpuModule JSON>, "link": "microwave"|"optical"|"direct",
///            "modules": "2,4,8,16" }`.
async fn qnet_scale(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let module = req.get("module").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: module (QpuModule JSON)"))?;
    let link = req.get("link").and_then(|v| v.as_str()).unwrap_or("microwave");
    let modules = req.get("modules").and_then(|v| v.as_str()).unwrap_or("2,4,8,16");
    let module_f = write_temp_json(&module)?;
    let result = run_tool("qnet", &[
        "--json", "scale",
        "--module", module_f.path().to_str().unwrap(),
        "--link", link, "--modules", modules,
    ])?;
    Ok(Json(result))
}

/// POST /qnet/compare-links — compare link technologies for a given module spec.
///
/// Accepts: `{ "module": <QpuModule JSON> }`.
async fn qnet_compare_links(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let module = req.get("module").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: module (QpuModule JSON)"))?;
    let module_f = write_temp_json(&module)?;
    let result = run_tool("qnet", &[
        "--json", "compare-links",
        "--module", module_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqchem endpoints
// ---------------------------------------------------------------------------

/// GET /qchem/health
async fn qchem_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qchem"}))
}

/// POST /qchem/molecule — map a molecular Hamiltonian to qubits.
///
/// Accepts: `{ "name": "h2"|"lih"|"beh2"|"h2o"|"n2",
///            "mapping": "jw"|"bk"|"parity",
///            "bond_length": float (Å, optional) }`.
async fn qchem_molecule(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let name = req.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: name (h2|lih|beh2|h2o|n2)"))?;
    let mapping = req.get("mapping").and_then(|v| v.as_str()).unwrap_or("jw");
    let basis = req.get("basis").and_then(|v| v.as_str()).unwrap_or("sto-3g");
    let mut args: Vec<String> = vec![
        "molecule".into(), "--json".into(),
        "--name".into(), name.into(),
        "--mapping".into(), mapping.into(),
        "--basis".into(), basis.into(),
    ];
    if let Some(bl) = req.get("bond_length").and_then(|v| v.as_f64()) {
        args.extend(["--bond-length".into(), bl.to_string()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qchem", &args_ref)?;
    Ok(Json(result))
}

/// POST /qchem/vqe — run VQE optimization for a molecule.
///
/// Accepts: `{ "name": str, "ansatz": "uccsd"|"hwe",
///            "optimizer": "cobyla"|"bfgs"|"adam"|"nelder_mead",
///            "max_iter": int, "bond_length": float }`.
async fn qchem_vqe(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let name = req.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: name"))?;
    let ansatz = req.get("ansatz").and_then(|v| v.as_str()).unwrap_or("uccsd");
    let optimizer = req.get("optimizer").and_then(|v| v.as_str()).unwrap_or("cobyla");
    let max_iter = req.get("max_iter").and_then(as_u64_loose).unwrap_or(200).to_string();
    let mut args: Vec<String> = vec![
        "vqe".into(), "--json".into(),
        "--name".into(), name.into(),
        "--ansatz".into(), ansatz.into(),
        "--optimizer".into(), optimizer.into(),
        "--max-iter".into(), max_iter,
    ];
    if let Some(bl) = req.get("bond_length").and_then(|v| v.as_f64()) {
        args.extend(["--bond-length".into(), bl.to_string()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qchem", &args_ref)?;
    Ok(Json(result))
}

/// POST /qchem/resources — estimate quantum resources for a molecule + ansatz.
///
/// Accepts: `{ "name": str, "mapping": str, "ansatz": str, "basis": str }`.
async fn qchem_resources(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let name = req.get("name").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: name"))?;
    let mapping = req.get("mapping").and_then(|v| v.as_str()).unwrap_or("jw");
    let ansatz = req.get("ansatz").and_then(|v| v.as_str()).unwrap_or("uccsd");
    let basis = req.get("basis").and_then(|v| v.as_str()).unwrap_or("sto-3g");
    let result = run_tool("qchem", &[
        "resources", "--json",
        "--name", name, "--mapping", mapping,
        "--ansatz", ansatz, "--basis", basis,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustycryo-wiring endpoints
// ---------------------------------------------------------------------------

/// GET /wiring/health
async fn wiring_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "wiring"}))
}

/// POST /wiring/design — design a full cryostat wiring for N qubits.
///
/// Accepts: `{ "qubits": int, "fridge": "BF-XLD"|"BF-LD"|"BF-SD" }`.
async fn wiring_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let qubits = req.get("qubits").and_then(as_u64_loose).unwrap_or(50).to_string();
    let mut args: Vec<String> = vec!["design".into(), "--json".into(), "--qubits".into(), qubits];
    if let Some(f) = req.get("fridge").and_then(|v| v.as_str()) {
        args.extend(["--fridge".into(), f.into()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("wiring", &args_ref)?;
    Ok(Json(result))
}

/// POST /wiring/noise — compute noise photon budget for a signal line type.
///
/// Accepts: `{ "line": "xy_drive"|"flux"|"readout"|"twpa_pump" }`.
async fn wiring_noise(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let line = req.get("line").and_then(|v| v.as_str()).unwrap_or("xy_drive");
    let result = run_tool("wiring", &["noise", "--json", "--line", line])?;
    Ok(Json(result))
}

/// POST /wiring/scale — scale analysis for multiple qubit counts.
///
/// Accepts: `{ "fridge": str, "qubits": "10,50,100,500" }`.
async fn wiring_scale(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let fridge = req.get("fridge").and_then(|v| v.as_str()).unwrap_or("BF-XLD");
    let mut args: Vec<String> = vec!["scale".into(), "--json".into(), "--fridge".into(), fridge.into()];
    if let Some(q) = req.get("qubits").and_then(|v| v.as_str()) {
        args.extend(["--qubits".into(), q.into()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("wiring", &args_ref)?;
    Ok(Json(result))
}

/// POST /wiring/optimize — optimize attenuation distribution for a line type.
///
/// Accepts: `{ "line": str, "target_noise": float }`.
async fn wiring_optimize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let line = req.get("line").and_then(|v| v.as_str()).unwrap_or("readout");
    let target = req.get("target_noise").and_then(|v| v.as_f64()).unwrap_or(0.01).to_string();
    let result = run_tool("wiring", &[
        "optimize", "--json", "--line", line, "--target-noise", &target,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyextract endpoints
// ---------------------------------------------------------------------------

/// GET /extract/health
async fn extract_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustyextract"}))
}

/// POST /extract/cpw — compute CPW transmission line parameters.
///
/// Accepts: `{ "width": float (µm), "gap": float (µm),
///            "length": float (µm), "epsilon_r": float }`.
async fn extract_cpw(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let width = req.get("width").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: width (µm)"))?.to_string();
    let gap = req.get("gap").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: gap (µm)"))?.to_string();
    let mut args: Vec<String> = vec![
        "--json".into(), "cpw".into(),
        "--width".into(), width,
        "--gap".into(), gap,
    ];
    if let Some(l) = req.get("length").and_then(|v| v.as_f64()) {
        args.extend(["--length".into(), l.to_string()]);
    }
    if let Some(eps) = req.get("epsilon_r").and_then(|v| v.as_f64()) {
        args.extend(["--epsilon-r".into(), eps.to_string()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("rustyextract", &args_ref)?;
    Ok(Json(result))
}

/// POST /extract/tls — predict TLS-limited T1 for a transmon cross geometry.
///
/// Accepts: `{ "cross_length": float (µm), "cross_width": float (µm),
///            "gap": float (µm), "freq": float (GHz),
///            "substrate": "silicon"|"sapphire"|"silicon-nitride",
///            "metal": "aluminum"|"niobium"|"tantalum"|"nbtin" }`.
async fn extract_tls(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let mut args: Vec<String> = vec!["--json".into(), "tls".into()];
    if let Some(v) = req.get("cross_length").and_then(|v| v.as_f64()) {
        args.extend(["--cross-length".into(), v.to_string()]);
    }
    if let Some(v) = req.get("cross_width").and_then(|v| v.as_f64()) {
        args.extend(["--cross-width".into(), v.to_string()]);
    }
    if let Some(v) = req.get("gap").and_then(|v| v.as_f64()) {
        args.extend(["--gap".into(), v.to_string()]);
    }
    if let Some(v) = req.get("freq").and_then(|v| v.as_f64()) {
        args.extend(["--freq".into(), v.to_string()]);
    }
    if let Some(v) = req.get("substrate").and_then(|v| v.as_str()) {
        args.extend(["--substrate".into(), v.into()]);
    }
    if let Some(v) = req.get("metal").and_then(|v| v.as_str()) {
        args.extend(["--metal".into(), v.into()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("rustyextract", &args_ref)?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqatom endpoints
// ---------------------------------------------------------------------------

/// GET /qatom/health
async fn qatom_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qatom"}))
}

/// POST /qatom/design — design an optical tweezer array.
///
/// Accepts: `{ "rows": int, "cols": int, "spacing": float (µm),
///            "species": "rb87"|"cs133"|"yb171"|"sr87"|"er166", "array": "square"|... }`.
async fn qatom_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let rows = req.get("rows").and_then(as_u64_loose).unwrap_or(10).to_string();
    let cols = req.get("cols").and_then(as_u64_loose).unwrap_or(10).to_string();
    let spacing = req.get("spacing").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("rb87");
    let array = req.get("array").and_then(|v| v.as_str()).unwrap_or("square");
    let result = run_tool("qatom", &[
        "design", "--json",
        "--rows", &rows, "--cols", &cols,
        "--spacing", &spacing, "--species", species, "--array", array,
    ])?;
    Ok(Json(result))
}

/// POST /qatom/gate — design a CZ gate between two atoms.
///
/// Accepts: `{ "species": str, "n_rydberg": int|float, "distance": float (µm) }`.
/// `n_rydberg` accepts a JSON float (truncated to int) so continuous-space
/// optimizers like `bayesian_outer_loop` can drive it without an integer
/// coercion step in the template.
async fn qatom_gate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("rb87");
    let n_rydberg = req
        .get("n_rydberg")
        .and_then(as_u64_loose)
        .unwrap_or(60)
        .to_string();
    let distance = req.get("distance").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let result = run_tool("qatom", &[
        "gate", "--json",
        "--species", species, "--n-rydberg", &n_rydberg, "--distance", &distance,
    ])?;
    Ok(Json(result))
}

/// POST /qatom/blockade — compute blockade radius vs Rydberg level.
///
/// Accepts: `{ "species": str, "n_levels": [int,...] }`.
async fn qatom_blockade(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("rb87");
    let mut args: Vec<String> = vec!["blockade".into(), "--json".into(), "--species".into(), species.into()];
    if let Some(levels) = req.get("n_levels").and_then(|v| v.as_array()) {
        let ls: Vec<String> = levels.iter().filter_map(|v| v.as_u64()).map(|n| n.to_string()).collect();
        if !ls.is_empty() {
            for l in &ls { args.push(l.clone()); }
            // insert --n-levels before the level values
            let insert_at = args.iter().position(|s| s == ls.first().unwrap()).unwrap();
            args.insert(insert_at, "--n-levels".into());
        }
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qatom", &args_ref)?;
    Ok(Json(result))
}

/// POST /qatom/loading — compute atom loading efficiency.
///
/// Accepts: `{ "trap_depth": float (mK), "temperature": float (µK) }`.
async fn qatom_loading(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let trap_depth = req.get("trap_depth").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let temperature = req.get("temperature").and_then(|v| v.as_f64()).unwrap_or(30.0).to_string();
    let result = run_tool("qatom", &[
        "loading", "--json",
        "--trap-depth", &trap_depth, "--temperature", &temperature,
    ])?;
    Ok(Json(result))
}

/// POST /qatom/multi-gate — design a multi-qubit Rydberg gate.
///
/// Accepts: `{ "species": str, "atoms": int, "gate": "toffoli"|"ccz" }`.
async fn qatom_multi_gate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("rb87");
    let atoms = req.get("atoms").and_then(as_u64_loose).unwrap_or(3).to_string();
    let gate = req.get("gate").and_then(|v| v.as_str()).unwrap_or("toffoli");
    let result = run_tool("qatom", &[
        "multi-gate", "--json",
        "--species", species, "--atoms", &atoms, "--gate", gate,
    ])?;
    Ok(Json(result))
}

/// POST /qatom/zone-layout — zone architecture layout for neutral-atom processors (phase 8H).
///
/// Accepts: `{ "n_storage": u32, "n_entangling": u32, "n_readout": u32, "n_reservoir": u32,
///             "transport_speed_um_per_us": f64, "zone_spacing_um": f64,
///             "gate_time_us": f64, "species": str }`.
async fn qatom_zone_layout(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_storage                = req.get("n_storage").and_then(as_u64_loose).unwrap_or(100);
    let n_entangling             = req.get("n_entangling").and_then(as_u64_loose).unwrap_or(10);
    let n_readout                = req.get("n_readout").and_then(as_u64_loose).unwrap_or(20);
    let n_reservoir              = req.get("n_reservoir").and_then(as_u64_loose).unwrap_or(50);
    let transport_speed_um_per_us = req.get("transport_speed_um_per_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let zone_spacing_um          = req.get("zone_spacing_um").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let gate_time_us             = req.get("gate_time_us").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let species                  = req.get("species").and_then(|v| v.as_str()).unwrap_or("Rb87").to_string();
    let arg_n_storage                 = format!("--n-storage={n_storage}");
    let arg_n_entangling              = format!("--n-entangling={n_entangling}");
    let arg_n_readout                 = format!("--n-readout={n_readout}");
    let arg_n_reservoir               = format!("--n-reservoir={n_reservoir}");
    let arg_transport_speed_um_per_us = format!("--transport-speed-um-per-us={transport_speed_um_per_us}");
    let arg_zone_spacing_um           = format!("--zone-spacing-um={zone_spacing_um}");
    let arg_gate_time_us              = format!("--gate-time-us={gate_time_us}");
    let arg_species                   = format!("--species={species}");
    let result = run_tool("qatom", &[
        "zone-layout", "--json",
        &arg_n_storage, &arg_n_entangling, &arg_n_readout, &arg_n_reservoir,
        &arg_transport_speed_um_per_us, &arg_zone_spacing_um, &arg_gate_time_us, &arg_species,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustypulse-qec endpoints
// ---------------------------------------------------------------------------

/// GET /pqec/health
async fn pqec_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "pqec"}))
}

/// POST /pqec/assess — assess whether gate parameters meet QEC thresholds.
///
/// Accepts: `{ "t1": float (µs), "t2": float (µs), "gate_time": float (ns),
///            "gate_fidelity": float }`.
async fn pqec_assess(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    // Default to canonical superconducting transmon coherence/gate numbers
    // (T1=80 µs, T2=60 µs, gate=200 ns @ 99.9% fidelity) so this endpoint
    // is usable as a smoke-test stage in pipelines like `qec_assessment`
    // where the upstream `surgery_resources` stage doesn't carry these
    // fields through.
    let t1 = req.get("t1").and_then(|v| v.as_f64()).unwrap_or(80.0).to_string();
    let t2 = req.get("t2").and_then(|v| v.as_f64()).unwrap_or(60.0).to_string();
    let gate_time = req.get("gate_time").and_then(|v| v.as_f64()).unwrap_or(200.0).to_string();
    let gate_fidelity = req.get("gate_fidelity").and_then(|v| v.as_f64()).unwrap_or(0.999).to_string();
    let result = run_tool("pqec", &[
        "--json", "assess",
        "--t1", &t1, "--t2", &t2,
        "--gate-time", &gate_time, "--gate-fidelity", &gate_fidelity,
    ])?;
    Ok(Json(result))
}

/// POST /pqec/threshold — evaluate a noise model against all QEC thresholds.
///
/// Accepts: `{ "noise_model": <NoiseModel JSON> }`.
async fn pqec_threshold(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let noise_model = req.get("noise_model").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: noise_model (NoiseModel JSON)"))?;
    let nm_f = write_temp_json(&noise_model)?;
    let result = run_tool("pqec", &[
        "--json", "threshold",
        "--noise-model", nm_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /pqec/overhead — estimate resource overhead for a target logical error rate.
///
/// Accepts: `{ "noise_model": <NoiseModel JSON>, "target_ler": float, "code": str }`.
async fn pqec_overhead(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let noise_model = req.get("noise_model").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: noise_model (NoiseModel JSON)"))?;
    let target_ler = req.get("target_ler").and_then(|v| v.as_f64()).unwrap_or(1e-12).to_string();
    let code = req.get("code").and_then(|v| v.as_str()).unwrap_or("surface");
    let nm_f = write_temp_json(&noise_model)?;
    let result = run_tool("pqec", &[
        "--json", "overhead",
        "--noise-model", nm_f.path().to_str().unwrap(),
        "--target-ler", &target_ler, "--code", code,
    ])?;
    Ok(Json(result))
}

/// POST /pqec/sweep — sweep gate times and report threshold analysis.
///
/// Accepts: `{ "t1": float (µs), "t2": float (µs), "gate_times": [float,...] (ns), "code": str }`.
async fn pqec_sweep(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let t1 = req.get("t1").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: t1 (µs)"))?.to_string();
    let t2 = req.get("t2").and_then(|v| v.as_f64())
        .ok_or_else(|| anyhow::anyhow!("missing field: t2 (µs)"))?.to_string();
    let code = req.get("code").and_then(|v| v.as_str()).unwrap_or("surface");
    let gate_times_s = if let Some(arr) = req.get("gate_times").and_then(|v| v.as_array()) {
        arr.iter().filter_map(|v| v.as_f64()).map(|f| f.to_string()).collect::<Vec<_>>().join(",")
    } else {
        "10,20,30,40,50,100,200".into()
    };
    let result = run_tool("pqec", &[
        "--json", "sweep",
        "--t1", &t1, "--t2", &t2,
        "--gate-times", &gate_times_s, "--code", code,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqspin endpoints
// ---------------------------------------------------------------------------

/// GET /qspin/health
async fn qspin_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qspin"}))
}

/// POST /qspin/design — design a silicon spin qubit dot array.
///
/// Accepts: `{ "layout": "linear"|"crossbar", "qubits": int, "rows": int,
///            "cols": int, "platform": "sige"|"simos"|"fdsoi"|"finfet" }`.
async fn qspin_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let layout = req.get("layout").and_then(|v| v.as_str()).unwrap_or("linear");
    let platform = req.get("platform").and_then(|v| v.as_str()).unwrap_or("sige");
    let mut args: Vec<String> = vec![
        "design".into(), "--json".into(),
        "--layout".into(), layout.into(),
        "--platform".into(), platform.into(),
    ];
    if layout == "crossbar" {
        let rows = req.get("rows").and_then(as_u64_loose).unwrap_or(2).to_string();
        let cols = req.get("cols").and_then(as_u64_loose).unwrap_or(4).to_string();
        args.extend(["--rows".into(), rows, "--cols".into(), cols]);
    } else {
        let qubits = req.get("qubits").and_then(as_u64_loose).unwrap_or(4).to_string();
        args.extend(["--qubits".into(), qubits]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qspin", &args_ref)?;
    Ok(Json(result))
}

/// POST /qspin/fidelity — compute silicon spin gate fidelity.
///
/// Accepts: `{ "array": <DotArray JSON>, "gate": "esr"|"edsr"|"exchange"|"cphase" }`.
async fn qspin_fidelity(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let array = req.get("array").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: array (DotArray JSON)"))?;
    let gate = req.get("gate").and_then(|v| v.as_str()).unwrap_or("exchange");
    let array_f = write_temp_json(&array)?;
    let result = run_tool("qspin", &[
        "fidelity", "--json",
        "--array", array_f.path().to_str().unwrap(),
        "--gate", gate,
    ])?;
    Ok(Json(result))
}

/// POST /qspin/stability — generate charge stability diagram.
///
/// Accepts: `{ "plunger_range": [min, max], "barrier_range": [min, max] }`.
async fn qspin_stability(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let mut args: Vec<String> = vec!["stability".into(), "--json".into()];
    if let Some(pr) = req.get("plunger_range").and_then(|v| v.as_array()) && pr.len() == 2 {
        let s = format!("{},{}", pr[0].as_f64().unwrap_or(-1.0), pr[1].as_f64().unwrap_or(1.0));
        args.extend(["--plunger-range".into(), s]);
    }
    if let Some(br) = req.get("barrier_range").and_then(|v| v.as_array()) && br.len() == 2 {
        let s = format!("{},{}", br[0].as_f64().unwrap_or(0.0), br[1].as_f64().unwrap_or(1.0));
        args.extend(["--barrier-range".into(), s]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qspin", &args_ref)?;
    Ok(Json(result))
}

/// POST /qspin/fab — generate GDS-II layout + DRC report.
///
/// Accepts: `{ "array": <DotArray JSON>, "platform": "sige"|"simos"|"fdsoi"|"finfet" }`.
async fn qspin_fab(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let array = req.get("array").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: array (DotArray JSON)"))?;
    let platform = req.get("platform").and_then(|v| v.as_str()).unwrap_or("sige");
    let array_f = write_temp_json(&array)?;
    let result = run_tool("qspin", &[
        "fab", "--json",
        "--array", array_f.path().to_str().unwrap(),
        "--platform", platform,
    ])?;
    Ok(Json(result))
}

/// POST /qspin/yield — Monte Carlo fabrication yield estimation.
///
/// Accepts: `{ "array": <DotArray JSON>, "platform": str,
///            "variation": float (%), "samples": int }`.
async fn qspin_yield(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let array = req.get("array").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: array (DotArray JSON)"))?;
    let platform = req.get("platform").and_then(|v| v.as_str()).unwrap_or("sige");
    let variation = req.get("variation").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let samples = req.get("samples").and_then(as_u64_loose).unwrap_or(10000).to_string();
    let array_f = write_temp_json(&array)?;
    let result = run_tool("qspin", &[
        "yield", "--json",
        "--array", array_f.path().to_str().unwrap(),
        "--platform", platform,
        "--variation", &variation,
        "--samples", &samples,
    ])?;
    Ok(Json(result))
}

/// POST /qspin/valley-split — valley splitting statistics for Si/SiGe spin qubits (phase 8G).
///
/// Accepts: `{ "n_dots": u32, "platform": str, "si_fraction_mean": f64,
///             "si_fraction_std": f64, "n_samples": u32,
///             "interface_width_nm": f64, "threshold_uev": f64 }`.
/// POST /qspin/pulse — silicon-spin gate-pulse budget (DRAG EDSR/ESR).
async fn qspin_pulse(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let rb = req.get("rabi_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let os = req.get("orbital_splitting_mev").and_then(|v| v.as_f64()).unwrap_or(1.5);
    let vs = req.get("valley_splitting_uev").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let cn = req.get("charge_noise_mhz").and_then(|v| v.as_f64()).unwrap_or(0.05);
    let no_drag = req.get("no_drag").and_then(|v| v.as_bool()).unwrap_or(false);
    let a_rb = format!("--rabi-mhz={rb}");
    let a_os = format!("--orbital-splitting-mev={os}");
    let a_vs = format!("--valley-splitting-uev={vs}");
    let a_cn = format!("--charge-noise-mhz={cn}");
    let mut args: Vec<&str> = vec!["pulse", "--json", &a_rb, &a_os, &a_vs, &a_cn];
    if no_drag {
        args.push("--no-drag");
    }
    let result = run_tool("qspin", &args)?;
    Ok(Json(result))
}

/// POST /qion/pulse — trapped-ion gate-pulse budget (MS shaping).
async fn qion_pulse(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let gr = req.get("gate_rabi_khz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let mf = req.get("mode_freq_khz").and_then(|v| v.as_f64()).unwrap_or(3000.0);
    let me = req.get("mode_freq_error_khz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let gt = req.get("gate_time_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let nm = req.get("n_modes").and_then(as_u64_loose).unwrap_or(5);
    let t2 = req.get("t2_us").and_then(|v| v.as_f64()).unwrap_or(1_000_000.0);
    let no_shaping = req.get("no_shaping").and_then(|v| v.as_bool()).unwrap_or(false);
    let a_gr = format!("--gate-rabi-khz={gr}");
    let a_mf = format!("--mode-freq-khz={mf}");
    let a_me = format!("--mode-freq-error-khz={me}");
    let a_gt = format!("--gate-time-us={gt}");
    let a_nm = format!("--n-modes={nm}");
    let a_t2 = format!("--t2-us={t2}");
    let mut args: Vec<&str> = vec!["pulse", "--json", &a_gr, &a_mf, &a_me, &a_gt, &a_nm, &a_t2];
    if no_shaping {
        args.push("--no-shaping");
    }
    let result = run_tool("qion", &args)?;
    Ok(Json(result))
}

/// POST /qatom/pulse — neutral-atom gate-pulse budget (Rydberg CZ shaping).
async fn qatom_pulse(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let rb = req.get("rabi_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let dt = req.get("detuning_mhz").and_then(|v| v.as_f64()).unwrap_or(500.0);
    let bl = req.get("blockade_mhz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let co = req.get("coherence_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let no_shaping = req.get("no_shaping").and_then(|v| v.as_bool()).unwrap_or(false);
    let a_rb = format!("--rabi-mhz={rb}");
    let a_dt = format!("--detuning-mhz={dt}");
    let a_bl = format!("--blockade-mhz={bl}");
    let a_co = format!("--coherence-us={co}");
    let mut args: Vec<&str> = vec!["pulse", "--json", &a_rb, &a_dt, &a_bl, &a_co];
    if no_shaping {
        args.push("--no-shaping");
    }
    let result = run_tool("qatom", &args)?;
    Ok(Json(result))
}

/// POST /qspin/frequency — silicon-spin Larmor-frequency plan (collision detection).
async fn qspin_frequency(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let nd = req.get("n_dots").and_then(as_u64_loose).unwrap_or(8);
    let bf = req.get("b_field_t").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let gf = req.get("g_factor").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let gr = req.get("gradient_mhz").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let rb = req.get("rabi_mhz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let a_nd = format!("--n-dots={nd}");
    let a_bf = format!("--b-field-t={bf}");
    let a_gf = format!("--g-factor={gf}");
    let a_gr = format!("--gradient-mhz={gr}");
    let a_rb = format!("--rabi-mhz={rb}");
    let result = run_tool("qspin", &["frequency", "--json", &a_nd, &a_bf, &a_gf, &a_gr, &a_rb])?;
    Ok(Json(result))
}

/// POST /qion/frequency — trapped-ion AOM tone plan (crowding + sidebands).
async fn qion_frequency(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ni = req.get("n_ions").and_then(as_u64_loose).unwrap_or(5);
    let sp = req.get("addressing_spacing_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let bw = req.get("aom_bandwidth_mhz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let mf = req.get("mode_freq_mhz").and_then(|v| v.as_f64()).unwrap_or(3.0);
    let bs = req.get("base_mhz").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let a_ni = format!("--n-ions={ni}");
    let a_sp = format!("--addressing-spacing-mhz={sp}");
    let a_bw = format!("--aom-bandwidth-mhz={bw}");
    let a_mf = format!("--mode-freq-mhz={mf}");
    let a_bs = format!("--base-mhz={bs}");
    let result = run_tool("qion", &["frequency", "--json", &a_ni, &a_sp, &a_bw, &a_mf, &a_bs])?;
    Ok(Json(result))
}

/// POST /qatom/frequency — neutral-atom addressing-frequency plan (crowding).
async fn qatom_frequency(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let nq = req.get("n_qubits").and_then(as_u64_loose).unwrap_or(10);
    let gr = req.get("addressing_gradient_mhz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let lw = req.get("addressing_linewidth_mhz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let a_nq = format!("--n-qubits={nq}");
    let a_gr = format!("--addressing-gradient-mhz={gr}");
    let a_lw = format!("--addressing-linewidth-mhz={lw}");
    let result = run_tool("qatom", &["frequency", "--json", &a_nq, &a_gr, &a_lw])?;
    Ok(Json(result))
}

/// POST /qspin/crosstalk — silicon-spin crosstalk budget (residual exchange + capacitive).
async fn qspin_crosstalk(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ex = req.get("exchange_residual_mhz").and_then(|v| v.as_f64()).unwrap_or(0.05);
    let cf = req.get("capacitive_crosstalk_fraction").and_then(|v| v.as_f64()).unwrap_or(0.02);
    let gs = req.get("gate_freq_shift_mhz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let gt = req.get("gate_time_ns").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let a_ex = format!("--exchange-residual-mhz={ex}");
    let a_cf = format!("--capacitive-crosstalk-fraction={cf}");
    let a_gs = format!("--gate-freq-shift-mhz={gs}");
    let a_gt = format!("--gate-time-ns={gt}");
    let result = run_tool("qspin", &["crosstalk", "--json", &a_ex, &a_cf, &a_gs, &a_gt])?;
    Ok(Json(result))
}

/// POST /qion/crosstalk — trapped-ion crosstalk budget (mode-mediated ZZ + Stark).
async fn qion_crosstalk(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let gr = req.get("gate_rabi_khz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let gd = req.get("gate_detuning_khz").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let ni = req.get("n_ions").and_then(as_u64_loose).unwrap_or(5);
    let af = req.get("addressing_crosstalk_fraction").and_then(|v| v.as_f64()).unwrap_or(0.01);
    let cr = req.get("carrier_rabi_khz").and_then(|v| v.as_f64()).unwrap_or(500.0);
    let gt = req.get("gate_time_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let a_gr = format!("--gate-rabi-khz={gr}");
    let a_gd = format!("--gate-detuning-khz={gd}");
    let a_ni = format!("--n-ions={ni}");
    let a_af = format!("--addressing-crosstalk-fraction={af}");
    let a_cr = format!("--carrier-rabi-khz={cr}");
    let a_gt = format!("--gate-time-us={gt}");
    let result = run_tool("qion", &["crosstalk", "--json", &a_gr, &a_gd, &a_ni, &a_af, &a_cr, &a_gt])?;
    Ok(Json(result))
}

/// POST /qatom/crosstalk — neutral-atom crosstalk budget (Rydberg vdW + addressing).
async fn qatom_crosstalk(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let sp = req.get("spacing_um").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let c6 = req.get("c6_ghz_um6").and_then(|v| v.as_f64()).unwrap_or(138.0);
    let aw = req.get("addressing_waist_um").and_then(|v| v.as_f64()).unwrap_or(1.5);
    let gr = req.get("gate_rabi_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let gt = req.get("gate_time_us").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let a_sp = format!("--spacing-um={sp}");
    let a_c6 = format!("--c6-ghz-um6={c6}");
    let a_aw = format!("--addressing-waist-um={aw}");
    let a_gr = format!("--gate-rabi-mhz={gr}");
    let a_gt = format!("--gate-time-us={gt}");
    let result = run_tool("qatom", &["crosstalk", "--json", &a_sp, &a_c6, &a_aw, &a_gr, &a_gt])?;
    Ok(Json(result))
}

/// POST /qspin/readout — silicon-spin single-shot readout budget (spin-to-charge).
async fn qspin_readout(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let t1_us = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(10000.0);
    let readout_time_us = req.get("readout_time_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let sensor_snr = req.get("sensor_snr").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let electron_temp_mk = req.get("electron_temp_mk").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let b_field_t = req.get("b_field_t").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let g_factor = req.get("g_factor").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let tunnel_rate_khz = req.get("tunnel_rate_khz").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let a_t = format!("--t1-us={t1_us}");
    let a_r = format!("--readout-time-us={readout_time_us}");
    let a_s = format!("--sensor-snr={sensor_snr}");
    let a_e = format!("--electron-temp-mk={electron_temp_mk}");
    let a_b = format!("--b-field-t={b_field_t}");
    let a_g = format!("--g-factor={g_factor}");
    let a_k = format!("--tunnel-rate-khz={tunnel_rate_khz}");
    let result = run_tool("qspin", &["readout", "--json", &a_t, &a_r, &a_s, &a_e, &a_b, &a_g, &a_k])?;
    Ok(Json(result))
}

/// POST /qion/readout — trapped-ion single-shot readout budget (fluorescence).
async fn qion_readout(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("yb171").to_string();
    let ce = req.get("collection_efficiency").and_then(|v| v.as_f64()).unwrap_or(0.03);
    let sr = req.get("scatter_rate_mhz").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let dt = req.get("detection_time_us").and_then(|v| v.as_f64()).unwrap_or(200.0);
    let dc = req.get("dark_count_rate_khz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let dp = req.get("depumping_time_ms").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let a_sp = format!("--species={species}");
    let a_ce = format!("--collection-efficiency={ce}");
    let a_sr = format!("--scatter-rate-mhz={sr}");
    let a_dt = format!("--detection-time-us={dt}");
    let a_dc = format!("--dark-count-rate-khz={dc}");
    let a_dp = format!("--depumping-time-ms={dp}");
    let result = run_tool("qion", &["readout", "--json", &a_sp, &a_ce, &a_sr, &a_dt, &a_dc, &a_dp])?;
    Ok(Json(result))
}

/// POST /qatom/readout — neutral-atom single-shot readout budget (fluorescence imaging).
async fn qatom_readout(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ce = req.get("collection_efficiency").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let sr = req.get("scatter_rate_mhz").and_then(|v| v.as_f64()).unwrap_or(30.0);
    let it = req.get("imaging_time_us").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let bg = req.get("background_rate_khz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let al = req.get("atom_loss_rate_khz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let a_ce = format!("--collection-efficiency={ce}");
    let a_sr = format!("--scatter-rate-mhz={sr}");
    let a_it = format!("--imaging-time-us={it}");
    let a_bg = format!("--background-rate-khz={bg}");
    let a_al = format!("--atom-loss-rate-khz={al}");
    let result = run_tool("qatom", &["readout", "--json", &a_ce, &a_sr, &a_it, &a_bg, &a_al])?;
    Ok(Json(result))
}

/// POST /qspin/coherence — silicon-spin coherence budget (T1/T2*/T2-echo by channel).
async fn qspin_coherence(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let g_factor = req.get("g_factor").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let b_field_t = req.get("b_field_t").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let charge_noise_uev = req.get("charge_noise_uev").and_then(|v| v.as_f64()).unwrap_or(2.0);
    let magnetic_noise_ut = req.get("magnetic_noise_ut").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let si29_fraction = req.get("si29_fraction").and_then(|v| v.as_f64()).unwrap_or(0.047);
    let valley_splitting_uev = req.get("valley_splitting_uev").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let a_g = format!("--g-factor={g_factor}");
    let a_b = format!("--b-field-t={b_field_t}");
    let a_c = format!("--charge-noise-uev={charge_noise_uev}");
    let a_m = format!("--magnetic-noise-ut={magnetic_noise_ut}");
    let a_s = format!("--si29-fraction={si29_fraction}");
    let a_v = format!("--valley-splitting-uev={valley_splitting_uev}");
    let result = run_tool("qspin", &["coherence", "--json", &a_g, &a_b, &a_c, &a_m, &a_s, &a_v])?;
    Ok(Json(result))
}

/// POST /qion/coherence — trapped-ion coherence budget (T1/T2*/T2-echo by channel).
async fn qion_coherence(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("yb171").to_string();
    let sens = req.get("magnetic_sensitivity_hz_per_nt").and_then(|v| v.as_f64()).unwrap_or(13.996);
    let noise = req.get("magnetic_noise_nt").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let laser = req.get("laser_linewidth_hz").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let heating = req.get("heating_quanta_per_s").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let a_sp = format!("--species={species}");
    let a_se = format!("--magnetic-sensitivity-hz-per-nt={sens}");
    let a_no = format!("--magnetic-noise-nt={noise}");
    let a_la = format!("--laser-linewidth-hz={laser}");
    let a_he = format!("--heating-quanta-per-s={heating}");
    let result = run_tool("qion", &["coherence", "--json", &a_sp, &a_se, &a_no, &a_la, &a_he])?;
    Ok(Json(result))
}

/// POST /qatom/coherence — neutral-atom coherence budget (T1/T2*/T2-echo by channel).
async fn qatom_coherence(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_principal = req.get("n_principal").and_then(as_u64_loose).unwrap_or(60);
    let temperature_uk = req.get("temperature_uk").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let bbr_temperature_k = req.get("bbr_temperature_k").and_then(|v| v.as_f64()).unwrap_or(300.0);
    let vacuum_lifetime_s = req.get("vacuum_lifetime_s").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let magnetic_noise_mg = req.get("magnetic_noise_mg").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let laser_tphi_us = req.get("laser_tphi_us").and_then(|v| v.as_f64()).unwrap_or(500.0);
    let ground_state = req.get("ground_state").and_then(|v| v.as_bool()).unwrap_or(false);
    let a_n = format!("--n-principal={n_principal}");
    let a_t = format!("--temperature-uk={temperature_uk}");
    let a_bbr = format!("--bbr-temperature-k={bbr_temperature_k}");
    let a_vac = format!("--vacuum-lifetime-s={vacuum_lifetime_s}");
    let a_mag = format!("--magnetic-noise-mg={magnetic_noise_mg}");
    let a_las = format!("--laser-tphi-us={laser_tphi_us}");
    let mut args: Vec<&str> = vec!["coherence", "--json", &a_n, &a_t, &a_bbr, &a_vac, &a_mag, &a_las];
    if ground_state {
        args.push("--ground-state");
    }
    let result = run_tool("qatom", &args)?;
    Ok(Json(result))
}

async fn qspin_valley_split(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_dots             = req.get("n_dots").and_then(as_u64_loose).unwrap_or(6);
    let platform           = req.get("platform").and_then(|v| v.as_str()).unwrap_or("sige").to_string();
    let si_fraction_mean   = req.get("si_fraction_mean").and_then(|v| v.as_f64()).unwrap_or(0.3);
    let si_fraction_std    = req.get("si_fraction_std").and_then(|v| v.as_f64()).unwrap_or(0.02);
    let n_samples          = req.get("n_samples").and_then(as_u64_loose).unwrap_or(1000);
    let interface_width_nm = req.get("interface_width_nm").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let threshold_uev      = req.get("threshold_uev").and_then(|v| v.as_f64()).unwrap_or(260.0);
    let arg_n_dots             = format!("--n-dots={n_dots}");
    let arg_platform           = format!("--platform={platform}");
    let arg_si_fraction_mean   = format!("--si-fraction-mean={si_fraction_mean}");
    let arg_si_fraction_std    = format!("--si-fraction-std={si_fraction_std}");
    let arg_n_samples          = format!("--n-samples={n_samples}");
    let arg_interface_width_nm = format!("--interface-width-nm={interface_width_nm}");
    let arg_threshold_uev      = format!("--threshold-uev={threshold_uev}");
    let result = run_tool("qspin", &[
        "valley-split", "--json",
        &arg_n_dots, &arg_platform, &arg_si_fraction_mean, &arg_si_fraction_std,
        &arg_n_samples, &arg_interface_width_nm, &arg_threshold_uev,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqion endpoints
// ---------------------------------------------------------------------------

/// GET /qion/health
async fn qion_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qion"}))
}

/// POST /qion/design — design a trapped-ion QCCD processor layout.
///
/// Accepts: `{ "type": "qccd"|"linear", "gate_zones": int,
///            "storage_zones": int, "species": "ca40"|"ba137"|"yb171"|"sr88"|"be9" }`.
async fn qion_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let trap_type = req.get("type").and_then(|v| v.as_str()).unwrap_or("qccd");
    let gate_zones = req.get("gate_zones").and_then(as_u64_loose).unwrap_or(4).to_string();
    let storage_zones = req.get("storage_zones").and_then(as_u64_loose).unwrap_or(8).to_string();
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("ca40");
    let result = run_tool("qion", &[
        "design", "--json",
        "--type", trap_type,
        "--gate-zones", &gate_zones,
        "--storage-zones", &storage_zones,
        "--species", species,
    ])?;
    Ok(Json(result))
}

/// POST /qion/ms-gate — compute Mølmer-Sørensen gate parameters.
///
/// Accepts: `{ "species": str, "mode_freq": float (MHz) }`.
async fn qion_ms_gate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("ca40");
    let mode_freq = req.get("mode_freq").and_then(|v| v.as_f64()).unwrap_or(3.0).to_string();
    let result = run_tool("qion", &[
        "ms-gate", "--json",
        "--species", species,
        "--mode-freq", &mode_freq,
    ])?;
    Ok(Json(result))
}

/// POST /qion/modes — compute motional modes of an ion chain.
///
/// Accepts: `{ "ions": int, "trap_freq": float (MHz), "species": str }`.
async fn qion_modes(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ions = req.get("ions").and_then(as_u64_loose).unwrap_or(5).to_string();
    let trap_freq = req.get("trap_freq").and_then(|v| v.as_f64()).unwrap_or(3.0).to_string();
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("ca40");
    let result = run_tool("qion", &[
        "modes", "--json",
        "--ions", &ions,
        "--trap-freq", &trap_freq,
        "--species", species,
    ])?;
    Ok(Json(result))
}

/// POST /qion/cooling — design sympathetic cooling configuration.
///
/// Accepts: `{ "qubit_species": str, "coolant_species": str }`.
async fn qion_cooling(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let qubit_species = req.get("qubit_species").and_then(|v| v.as_str()).unwrap_or("yb171");
    let coolant_species = req.get("coolant_species").and_then(|v| v.as_str()).unwrap_or("be9");
    let result = run_tool("qion", &[
        "cooling", "--json",
        "--qubit-species", qubit_species,
        "--coolant-species", coolant_species,
    ])?;
    Ok(Json(result))
}

/// POST /qion/schedule — schedule a circuit on a QCCD trap.
///
/// Accepts: `{ "circuit": [[qa,qb],...] (list of 2Q gate pairs), "trap": <TrapGeometry JSON> }`.
async fn qion_schedule(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit (list of [qa,qb] pairs)"))?;
    let trap = req.get("trap").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: trap (TrapGeometry JSON)"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let trap_f = write_temp_json(&trap)?;
    let result = run_tool("qion", &[
        "schedule", "--json",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--trap", trap_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustybosonic endpoints
// ---------------------------------------------------------------------------

/// GET /bosonic/health
async fn bosonic_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustybosonic"}))
}

/// POST /bosonic/simulate — simulate bosonic code time evolution.
///
/// Accepts: `{ "code": "cat"|"gkp"|"binomial", "alpha": float, "delta": float,
///            "kappa": float, "time": int, "n_max": int }`.
async fn bosonic_simulate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let code = req.get("code").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: code (cat, gkp, binomial)"))?;
    let alpha_s = req.get("alpha").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let delta_s = req.get("delta").and_then(|v| v.as_f64()).unwrap_or(0.3).to_string();
    let kappa_s = req.get("kappa").and_then(|v| v.as_f64()).unwrap_or(1e-3).to_string();
    let time_s = req.get("time").and_then(as_u64_loose).unwrap_or(100).to_string();
    let n_max_s = req.get("n_max").and_then(as_u64_loose).unwrap_or(30).to_string();
    let result = run_tool("rustybosonic", &[
        "--json", "simulate",
        "--code", code,
        "--alpha", &alpha_s,
        "--delta", &delta_s,
        "--kappa", &kappa_s,
        "--time", &time_s,
        "--n-max", &n_max_s,
    ])?;
    Ok(Json(result))
}

/// POST /bosonic/compare — compare logical error rates across bosonic codes.
///
/// Accepts: `{ "codes": "cat,gkp,binomial", "kappa": float, "n_max": int }`.
async fn bosonic_compare(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let codes = req.get("codes").and_then(|v| v.as_str()).unwrap_or("cat,gkp,binomial");
    let kappa_s = req.get("kappa").and_then(|v| v.as_f64()).unwrap_or(1e-3).to_string();
    let n_max_s = req.get("n_max").and_then(as_u64_loose).unwrap_or(30).to_string();
    let result = run_tool("rustybosonic", &[
        "--json", "compare",
        "--codes", codes,
        "--kappa", &kappa_s,
        "--n-max", &n_max_s,
    ])?;
    Ok(Json(result))
}

/// POST /bosonic/optimize — find optimal code parameters minimizing logical error rate.
///
/// Accepts: `{ "code": "cat"|"gkp", "kappa": float, "target_ler": float }`.
async fn bosonic_optimize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let code = req.get("code").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: code (cat, gkp)"))?;
    let kappa_s = req.get("kappa").and_then(|v| v.as_f64()).unwrap_or(1e-3).to_string();
    let ler_s = req.get("target_ler").and_then(|v| v.as_f64()).unwrap_or(1e-6).to_string();
    let result = run_tool("rustybosonic", &[
        "--json", "optimize",
        "--code", code,
        "--kappa", &kappa_s,
        "--target-ler", &ler_s,
    ])?;
    Ok(Json(result))
}

/// POST /bosonic/break-even — compute T1 ratio at break-even point.
///
/// Accepts: `{ "code": "cat"|"gkp"|"binomial", "alpha": float, "t1": float, "kappa": float }`.
async fn bosonic_break_even(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let code = req.get("code").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: code (cat, gkp, binomial)"))?;
    let alpha_s = req.get("alpha").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let delta_s = req.get("delta").and_then(|v| v.as_f64()).unwrap_or(0.3).to_string();
    let t1_s = req.get("t1").and_then(|v| v.as_f64()).unwrap_or(100.0).to_string();
    let kappa_s = req.get("kappa").and_then(|v| v.as_f64()).unwrap_or(1e-3).to_string();
    let n_max_s = req.get("n_max").and_then(as_u64_loose).unwrap_or(30).to_string();
    let result = run_tool("rustybosonic", &[
        "--json", "break-even",
        "--code", code,
        "--alpha", &alpha_s,
        "--delta", &delta_s,
        "--t1", &t1_s,
        "--kappa", &kappa_s,
        "--n-max", &n_max_s,
    ])?;
    Ok(Json(result))
}

/// POST /bosonic/concat — concatenated bosonic QEC (phase 8I).
///
/// Accepts: `{ "inner_code": str, "outer_code": str, "alpha": f64, "delta": f64,
///             "outer_distance": u32, "kappa": f64, "t1_us": f64 }`.
async fn bosonic_concat(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let inner_code     = req.get("inner_code").and_then(|v| v.as_str()).unwrap_or("cat").to_string();
    let outer_code     = req.get("outer_code").and_then(|v| v.as_str()).unwrap_or("repetition").to_string();
    let alpha          = req.get("alpha").and_then(|v| v.as_f64()).unwrap_or(3.0);
    let delta          = req.get("delta").and_then(|v| v.as_f64()).unwrap_or(0.3);
    let outer_distance = req.get("outer_distance").and_then(as_u64_loose).unwrap_or(5);
    let kappa          = req.get("kappa").and_then(|v| v.as_f64()).unwrap_or(0.001);
    let t1_us          = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let arg_inner_code     = format!("--inner-code={inner_code}");
    let arg_outer_code     = format!("--outer-code={outer_code}");
    let arg_alpha          = format!("--alpha={alpha}");
    let arg_delta          = format!("--delta={delta}");
    let arg_outer_distance = format!("--outer-distance={outer_distance}");
    let arg_kappa          = format!("--kappa={kappa}");
    let arg_t1_us          = format!("--t1-us={t1_us}");
    let result = run_tool("rustybosonic", &[
        "concat-bosonic", "--json",
        &arg_inner_code, &arg_outer_code, &arg_alpha, &arg_delta,
        &arg_outer_distance, &arg_kappa, &arg_t1_us,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustycodesign endpoints
// ---------------------------------------------------------------------------

/// GET /codesign/health
async fn codesign_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "codesign"}))
}

/// POST /qec/compile — cross-platform QEC compiler. Body: `{ platform, physical_error_rate?,
/// target_ler? }`. Recommends a code + distance + decoder per modality (ion/atom can run
/// non-planar qLDPC; SC/spin fall back to surface/floquet) with physical-qubit overhead.
async fn qec_compile(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let platform = req
        .get("platform")
        .and_then(|v| v.as_str())
        .unwrap_or("superconducting")
        .to_string();
    let per = req.get("physical_error_rate").and_then(|v| v.as_f64()).unwrap_or(1e-3);
    let ler = req.get("target_ler").and_then(|v| v.as_f64()).unwrap_or(1e-9);
    let a_p = format!("--platform={platform}");
    let a_e = format!("--physical-error-rate={per}");
    let a_l = format!("--target-ler={ler}");
    let result = run_tool("codesign", &["qec-compile", "--json", &a_p, &a_e, &a_l])?;
    Ok(Json(result))
}

/// POST /codesign/optimize — find optimal hardware/QEC co-design for an application.
///
/// Accepts: `{ "app": "chemistry"|"factoring"|"optimization"|"ml"|"simulation",
///            "molecule"?, "key_bits"?, "problem"?, "n_vars"?,
///            "max_qubits"?, "max_cost"?, "accuracy"? }`.
async fn codesign_optimize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let app = req.get("app").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: app (chemistry, factoring, optimization, ml, simulation)"))?;
    let mut args = vec!["--json", "optimize", "--app", app];
    let molecule; let basis; let key_bits_s; let problem; let n_vars_s;
    let max_qubits_s; let max_cost_s; let accuracy_s;
    if let Some(v) = req.get("molecule").and_then(|v| v.as_str()) { molecule = v.to_string(); args.extend(["--molecule", &molecule]); }
    if let Some(v) = req.get("basis").and_then(|v| v.as_str()) { basis = v.to_string(); args.extend(["--basis", &basis]); }
    if let Some(v) = req.get("key_bits").and_then(as_u64_loose) { key_bits_s = v.to_string(); args.extend(["--key-bits", &key_bits_s]); }
    if let Some(v) = req.get("problem").and_then(|v| v.as_str()) { problem = v.to_string(); args.extend(["--problem", &problem]); }
    if let Some(v) = req.get("n_vars").and_then(as_u64_loose) { n_vars_s = v.to_string(); args.extend(["--n-vars", &n_vars_s]); }
    if let Some(v) = req.get("max_qubits").and_then(as_u64_loose) { max_qubits_s = v.to_string(); args.extend(["--max-qubits", &max_qubits_s]); }
    if let Some(v) = req.get("max_cost").and_then(|v| v.as_f64()) { max_cost_s = v.to_string(); args.extend(["--max-cost", &max_cost_s]); }
    if let Some(v) = req.get("accuracy").and_then(|v| v.as_f64()) { accuracy_s = v.to_string(); args.extend(["--accuracy", &accuracy_s]); }
    let result = run_tool("codesign", &args)?;
    Ok(Json(result))
}

/// POST /codesign/roadmap — project hardware requirements across technology generations.
///
/// Accepts: `{ "app": str, "molecule"?, "key_bits"?, "generations"? }`.
async fn codesign_roadmap(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let app = req.get("app").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: app"))?;
    let generations_s = req.get("generations").and_then(as_u64_loose).unwrap_or(5).to_string();
    let mut args = vec!["--json", "roadmap", "--app", app, "--generations", &generations_s];
    let molecule; let key_bits_s;
    if let Some(v) = req.get("molecule").and_then(|v| v.as_str()) { molecule = v.to_string(); args.extend(["--molecule", &molecule]); }
    if let Some(v) = req.get("key_bits").and_then(as_u64_loose) { key_bits_s = v.to_string(); args.extend(["--key-bits", &key_bits_s]); }
    let result = run_tool("codesign", &args)?;
    Ok(Json(result))
}

/// POST /codesign/compare-platforms — compare platforms for an application.
///
/// Accepts: `{ "app": str, "problem"?, "n_vars"?, "molecule"?, "key_bits"? }`.
async fn codesign_compare_platforms(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let app = req.get("app").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: app"))?;
    let mut args = vec!["--json", "compare-platforms", "--app", app];
    let problem; let n_vars_s; let molecule; let key_bits_s;
    if let Some(v) = req.get("problem").and_then(|v| v.as_str()) { problem = v.to_string(); args.extend(["--problem", &problem]); }
    if let Some(v) = req.get("n_vars").and_then(as_u64_loose) { n_vars_s = v.to_string(); args.extend(["--n-vars", &n_vars_s]); }
    if let Some(v) = req.get("molecule").and_then(|v| v.as_str()) { molecule = v.to_string(); args.extend(["--molecule", &molecule]); }
    if let Some(v) = req.get("key_bits").and_then(as_u64_loose) { key_bits_s = v.to_string(); args.extend(["--key-bits", &key_bits_s]); }
    let result = run_tool("codesign", &args)?;
    Ok(Json(result))
}

/// POST /codesign/what-if — apply a single parameter change to an existing design.
///
/// Accepts: `{ "design": <CoDesignPoint JSON>, "change": "param=value" }`.
async fn codesign_what_if(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let design = req.get("design").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: design (CoDesignPoint JSON)"))?;
    let change = req.get("change").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: change (e.g. t1=200)"))?;
    let design_f = write_temp_json(&design)?;
    let result = run_tool("codesign", &[
        "--json", "what-if",
        "--design", design_f.path().to_str().unwrap(),
        "--change", change,
    ])?;
    Ok(Json(result))
}

/// POST /codesign/sensitivity — sweep a parameter and observe design feasibility changes.
///
/// Accepts: `{ "design": <CoDesignPoint JSON>, "param": str, "range": "min,max", "steps"?: int }`.
async fn codesign_sensitivity(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let design = req.get("design").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: design (CoDesignPoint JSON)"))?;
    let param = req.get("param").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: param"))?;
    let range = req.get("range").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: range (e.g. 100,10000)"))?;
    let steps_s = req.get("steps").and_then(as_u64_loose).unwrap_or(10).to_string();
    let design_f = write_temp_json(&design)?;
    let result = run_tool("codesign", &[
        "--json", "sensitivity",
        "--design", design_f.path().to_str().unwrap(),
        "--param", param,
        "--range", range,
        "--steps", &steps_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyqopt (QAOA) endpoints
// ---------------------------------------------------------------------------

/// GET /qaoa/health
async fn qaoa_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qopt"}))
}

/// POST /qaoa/maxcut — solve MaxCut via QAOA.
///
/// Accepts: `{ "nodes": int, "edge_prob": float, "p_layers": int }`.
async fn qaoa_maxcut(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let nodes_s = req.get("nodes").and_then(as_u64_loose).unwrap_or(10).to_string();
    let edge_prob_s = req.get("edge_prob").and_then(|v| v.as_f64()).unwrap_or(0.5).to_string();
    let p_s = req.get("p_layers").and_then(as_u64_loose).unwrap_or(1).to_string();
    let result = run_tool("qopt", &[
        "maxcut", "--json",
        "--nodes", &nodes_s,
        "--edge-prob", &edge_prob_s,
        "--p-layers", &p_s,
    ])?;
    Ok(Json(result))
}

/// POST /qaoa/portfolio — solve portfolio optimization via QAOA.
///
/// Accepts: `{ "assets": int, "risk": float, "budget": int, "p_layers": int }`.
async fn qaoa_portfolio(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let assets_s = req.get("assets").and_then(as_u64_loose).unwrap_or(5).to_string();
    let risk_s = req.get("risk").and_then(|v| v.as_f64()).unwrap_or(0.5).to_string();
    let budget_s = req.get("budget").and_then(as_u64_loose).unwrap_or(2).to_string();
    let p_s = req.get("p_layers").and_then(as_u64_loose).unwrap_or(1).to_string();
    let result = run_tool("qopt", &[
        "portfolio", "--json",
        "--assets", &assets_s,
        "--risk", &risk_s,
        "--budget", &budget_s,
        "--p-layers", &p_s,
    ])?;
    Ok(Json(result))
}

/// POST /qaoa/tsp — solve Travelling Salesman Problem via QAOA.
///
/// Accepts: `{ "cities": int, "p_layers": int }`.
async fn qaoa_tsp(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let cities_s = req.get("cities").and_then(as_u64_loose).unwrap_or(4).to_string();
    let p_s = req.get("p_layers").and_then(as_u64_loose).unwrap_or(1).to_string();
    let result = run_tool("qopt", &[
        "tsp", "--json",
        "--cities", &cities_s,
        "--p-layers", &p_s,
    ])?;
    Ok(Json(result))
}

/// POST /qaoa/resources — estimate QAOA circuit resources without execution.
///
/// Accepts: `{ "problem": "maxcut"|"portfolio"|"tsp", "nodes": int, "p_layers": int }`.
async fn qaoa_resources(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let problem = req.get("problem").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: problem (maxcut, portfolio, tsp)"))?;
    let nodes_s = req.get("nodes").and_then(as_u64_loose).unwrap_or(10).to_string();
    let p_s = req.get("p_layers").and_then(as_u64_loose).unwrap_or(1).to_string();
    let result = run_tool("qopt", &[
        "resources", "--json",
        "--problem", problem,
        "--nodes", &nodes_s,
        "--p-layers", &p_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// symclaw endpoints (symbolic math engine)
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// rustyqfw endpoints
// ---------------------------------------------------------------------------

/// GET /qfw/health
async fn qfw_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "qfw"}))
}

/// POST /qfw/compile — compile a quantum circuit to a pulse schedule.
///
/// Accepts: `{ "circuit": <circuit JSON>, "calibration": <calibration JSON> }`.
async fn qfw_compile(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let calibration = req.get("calibration").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: calibration"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let cal_f = write_temp_json(&calibration)?;
    let result = run_tool("qfw", &[
        "--json", "compile",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--calibration", cal_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /qfw/schedule — compile with dynamical decoupling insertion.
///
/// Accepts: `{ "circuit": <circuit JSON>, "dd": "xy4"|"cpmg"|"kdd" }`.
async fn qfw_schedule(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let mut args = vec![
        "--json", "schedule",
        "--circuit", circuit_f.path().to_str().unwrap(),
    ];
    let dd_str;
    if let Some(dd) = req.get("dd").and_then(|v| v.as_str()) {
        dd_str = dd.to_string();
        args.push("--dd");
        args.push(&dd_str);
    }
    let result = run_tool("qfw", &args)?;
    Ok(Json(result))
}

/// POST /qfw/simulate — simulate a compiled schedule.
///
/// Accepts: `{ "schedule": <schedule JSON>, "shots": int }`.
async fn qfw_simulate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let schedule = req.get("schedule").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: schedule"))?;
    let schedule_f = write_temp_json(&schedule)?;
    let shots_s = req.get("shots").and_then(as_u64_loose).unwrap_or(1000).to_string();
    let result = run_tool("qfw", &[
        "--json", "simulate",
        "--schedule", schedule_f.path().to_str().unwrap(),
        "--shots", &shots_s,
    ])?;
    Ok(Json(result))
}

/// POST /qfw/export — export a schedule to OpenQASM 3 or OpenPulse.
///
/// Accepts: `{ "schedule": <schedule JSON>, "format": "openqasm3"|"openpulse" }`.
async fn qfw_export(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let schedule = req.get("schedule").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: schedule"))?;
    let schedule_f = write_temp_json(&schedule)?;
    let format = req.get("format").and_then(|v| v.as_str()).unwrap_or("openqasm3");
    let result = run_tool("qfw", &[
        "--json", "export",
        "--schedule", schedule_f.path().to_str().unwrap(),
        "--format", format,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustytranspile endpoints
// ---------------------------------------------------------------------------

/// GET /transpile/health
async fn transpile_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "transpile"}))
}

/// POST /transpile/compile — transpile a circuit to a target hardware topology.
///
/// Accepts: `{ "circuit": <circuit JSON>, "target": "ibm_heavy_hex"|"google_sycamore"|"linear"|"all_to_all",
///            "optimization": 0-3, "target_size": int }`.
async fn transpile_compile(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let target = req.get("target").and_then(|v| v.as_str()).unwrap_or("ibm_heavy_hex");
    let opt_s = req.get("optimization").and_then(as_u64_loose).unwrap_or(2).to_string();
    let size_s = req.get("target_size").and_then(as_u64_loose).unwrap_or(20).to_string();
    let result = run_tool("transpile", &[
        "--json", "compile",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--target", target,
        "--optimization", &opt_s,
        "--target_size", &size_s,
    ])?;
    Ok(Json(result))
}

/// POST /transpile/analyze — analyze circuit structure (depth, gate counts, 2Q pairs).
///
/// Accepts: `{ "circuit": <circuit JSON> }`.
async fn transpile_analyze(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let result = run_tool("transpile", &[
        "--json", "analyze",
        "--circuit", circuit_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /transpile/noise-aware — noise-aware transpilation using chip characterization.
///
/// Accepts: `{ "circuit": <circuit JSON>, "noise": <noise JSON>, "target": str, "target_size": int }`.
async fn transpile_noise_aware(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let noise = req.get("noise").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: noise"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let noise_f = write_temp_json(&noise)?;
    let target = req.get("target").and_then(|v| v.as_str()).unwrap_or("ibm_heavy_hex");
    let size_s = req.get("target_size").and_then(as_u64_loose).unwrap_or(20).to_string();
    let result = run_tool("transpile", &[
        "--json", "noise-aware",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--noise", noise_f.path().to_str().unwrap(),
        "--target", target,
        "--target_size", &size_s,
    ])?;
    Ok(Json(result))
}

/// POST /transpile/compare — compare transpilation across multiple targets.
///
/// Accepts: `{ "circuit": <circuit JSON>, "targets": "ibm,google,linear", "optimization": 0-3 }`.
async fn transpile_compare(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = req.get("circuit").cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: circuit"))?;
    let circuit_f = write_temp_json(&circuit)?;
    let targets = req.get("targets").and_then(|v| v.as_str()).unwrap_or("ibm,google,linear");
    let opt_s = req.get("optimization").and_then(as_u64_loose).unwrap_or(2).to_string();
    let size_s = req.get("target_size").and_then(as_u64_loose).unwrap_or(20).to_string();
    let result = run_tool("transpile", &[
        "--json", "compare",
        "--circuit", circuit_f.path().to_str().unwrap(),
        "--targets", targets,
        "--optimization", &opt_s,
        "--target_size", &size_s,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustypkg endpoints
// ---------------------------------------------------------------------------

/// GET /pkg/health
async fn pkg_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustypkg"}))
}

/// POST /pkg/design — design a complete sample holder assembly.
///
/// Accepts either:
/// - A full `SampleHolderAssembly` JSON (must contain a `"housing"` key), or
/// - Simple scalar params: `housing_length_mm`, `housing_width_mm`,
///   `housing_height_mm`, `n_sma_ports`, `chip_length_mm`, `chip_width_mm`.
///
/// Calls `rustypkg --json design --config <tmpfile>` and returns the JSON output.
async fn pkg_design_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    // Determine assembly JSON
    let assembly = if req.get("housing").is_some() {
        // Caller passed a full SampleHolderAssembly
        req.clone()
    } else {
        // Build a minimal assembly from simple scalar params
        let hl = req.get("housing_length_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: housing_length_mm or housing"))?;
        let hw = req.get("housing_width_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: housing_width_mm"))?;
        let hh = req.get("housing_height_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: housing_height_mm"))?;
        let n_sma = req.get("n_sma_ports").and_then(as_u64_loose).unwrap_or(4) as usize;
        let cl = req.get("chip_length_mm").and_then(|v| v.as_f64()).unwrap_or(hl * 0.7);
        let cw = req.get("chip_width_mm").and_then(|v| v.as_f64()).unwrap_or(hw * 0.7);

        // Wall/lid thickness defaults
        let wall_mm = 5.0_f64;
        let lid_mm = 3.0_f64;
        let internal_l = (hl - 2.0 * wall_mm) / 1000.0;
        let internal_w = (hw - 2.0 * wall_mm) / 1000.0;
        let internal_d = (hh - lid_mm) / 1000.0;

        // Build SMA connectors on alternating North/South faces
        let faces = ["North", "South", "East", "West"];
        let sma_y_center = hw / 2000.0; // center in meters
        let sma_x_center = hl / 2000.0;
        let sma_connectors: Vec<Value> = (0..n_sma).map(|i| {
            let face = faces[i % 4];
            let pos = match face {
                "North" | "South" => json!({"x": sma_x_center, "y": 0.0}),
                _ => json!({"x": 0.0, "y": sma_y_center}),
            };
            json!({
                "position_m": pos,
                "face": face,
                "launch_length_m": 0.003,
                "launch_width_m": 0.0003,
                "connector_type": "ThroughWall"
            })
        }).collect();

        json!({
            "housing": {
                "internal_length_m": internal_l,
                "internal_width_m": internal_w,
                "internal_depth_m": internal_d,
                "wall_thickness_m": wall_mm / 1000.0,
                "lid_thickness_m": lid_mm / 1000.0,
                "chip_recess_depth_m": 0.0005,
                "chip_recess_length_m": cl / 1000.0,
                "chip_recess_width_m": cw / 1000.0,
                "material": "Aluminum6061"
            },
            "sma_connectors": sma_connectors,
            "wirebonds": null,
            "indium_seal": null,
            "screw_pattern": null
        })
    };

    let config_f = write_temp_json(&assembly)?;
    let result = run_tool("rustypkg", &[
        "--json", "design",
        "--config", config_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /pkg/box-modes — check housing box modes against the qubit band.
///
/// Accepts either:
/// - A full `HousingBoxParams` JSON (must contain `"internal_length_m"` key), or
/// - Simple scalar params: `housing_length_mm`, `housing_width_mm`, `housing_height_mm`,
///   plus optional `band_low_ghz`, `band_high_ghz`, `n_modes`.
async fn pkg_box_modes_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let housing = if req.get("internal_length_m").is_some() {
        req.clone()
    } else {
        let hl = req.get("housing_length_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: internal_length_m or housing_length_mm"))?;
        let hw = req.get("housing_width_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: housing_width_mm"))?;
        let hh = req.get("housing_height_mm").and_then(|v| v.as_f64())
            .ok_or_else(|| anyhow::anyhow!("missing field: housing_height_mm"))?;
        let wall_mm = 5.0_f64;
        let lid_mm = 3.0_f64;
        json!({
            "internal_length_m": (hl - 2.0 * wall_mm) / 1000.0,
            "internal_width_m": (hw - 2.0 * wall_mm) / 1000.0,
            "internal_depth_m": (hh - lid_mm) / 1000.0,
            "wall_thickness_m": wall_mm / 1000.0,
            "lid_thickness_m": lid_mm / 1000.0,
            "chip_recess_depth_m": 0.0005,
            "chip_recess_length_m": 0.01,
            "chip_recess_width_m": 0.008,
            "material": "Aluminum6061"
        })
    };

    let band_low_s = req.get("band_low_ghz").and_then(|v| v.as_f64()).unwrap_or(4.0).to_string();
    let band_high_s = req.get("band_high_ghz").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let n_modes_s = req.get("n_modes").and_then(as_u64_loose).unwrap_or(20).to_string();

    let config_f = write_temp_json(&housing)?;
    let result = run_tool("rustypkg", &[
        "--json", "box-modes",
        "--config", config_f.path().to_str().unwrap(),
        "--band-low", &band_low_s,
        "--band-high", &band_high_s,
        "--n-modes", &n_modes_s,
    ])?;
    Ok(Json(result))
}

/// POST /pkg/wirebonds — design wirebond connections between chip pads and PCB pads.
///
/// Accepts a `WirebondPadParams` JSON body with fields:
/// `chip_pads`, `pcb_pads`, `wire_diameter_m`, `wire_loop_height_m`, `max_wire_length_m`.
///
/// Calls `rustypkg --json wirebonds --config <tmpfile>` and returns JSON output.
async fn pkg_wirebonds_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    // Require chip_pads to be present — fail fast before touching the filesystem.
    if req.get("chip_pads").is_none() {
        return Err(anyhow::anyhow!("missing required field: chip_pads").into());
    }
    let config_f = write_temp_json(&req)?;
    let result = run_tool("rustypkg", &[
        "--json", "wirebonds",
        "--config", config_f.path().to_str().unwrap(),
    ])?;
    Ok(Json(result))
}

/// POST /pkg/export — export a `SampleHolderAssembly` to one or more CAD formats.
///
/// Accepts:
/// - `assembly`: full `SampleHolderAssembly` JSON (required)
/// - `formats`: optional array of strings e.g. `["gds","stl","dxf"]` (default: all three)
/// - `out_prefix`: optional output file prefix string (default: `/tmp/pkg_export`)
///
/// Calls `rustypkg --json export --config <tmpfile> --out-prefix <prefix> --formats <csv>`
/// and returns JSON output.
async fn pkg_export_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let assembly = req
        .get("assembly")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing required field: assembly"))?;

    let out_prefix = req
        .get("out_prefix")
        .and_then(|v| v.as_str())
        .unwrap_or("/tmp/pkg_export");

    let formats_csv = match req.get("formats") {
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<&str>>()
            .join(","),
        _ => "gds,stl,dxf,step".to_owned(),
    };

    let config_f = write_temp_json(&assembly)?;
    let result = run_tool("rustypkg", &[
        "--json", "export",
        "--config", config_f.path().to_str().unwrap(),
        "--out-prefix", out_prefix,
        "--formats", &formats_csv,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// rustyswap endpoints
// ---------------------------------------------------------------------------

/// GET /swap/health
async fn swap_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustyswap"}))
}

/// POST /swap/figure1d — SWAP inefficiency vs swap time sweep.
/// Reproduces Fig. 1d of Mollenhauer et al. arXiv:2407.16743.
async fn swap_figure1d(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let tau_b = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1_a1 = req.get("t1_a1_us").and_then(|v| v.as_f64()).unwrap_or(62.0).to_string();
    let t2_a1 = req.get("t2_a1_us").and_then(|v| v.as_f64()).unwrap_or(22.0).to_string();
    let t1_a2 = req.get("t1_a2_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2_a2 = req.get("t2_a2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let t_max = req.get("t_max_ns").and_then(|v| v.as_f64()).unwrap_or(400.0).to_string();
    let dt    = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let delta = req.get("delta_mhz").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let delta_str = format!("--delta-mhz={delta}");
    let n_fock = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let result = run_tool("rustyswap", &[
        "figure1d", "--json",
        "--omega-mhz", &omega, "--tau-b", &tau_b,
        "--t1-a1", &t1_a1, "--t2-a1", &t2_a1,
        "--t1-a2", &t1_a2, "--t2-a2", &t2_a2,
        &delta_str, "--t-max-ns", &t_max, "--dt-ns", &dt, "--n-fock", &n_fock,
    ])?;
    Ok(Json(result))
}

/// POST /swap/figure3a — Continuous Raman dynamics with Table I parameters.
/// Reproduces Fig. 3a of Mollenhauer et al.
async fn swap_figure3a(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega  = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let t_max  = req.get("t_max_ns").and_then(|v| v.as_f64()).unwrap_or(400.0).to_string();
    let dt     = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let n_fock = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let result = run_tool("rustyswap", &[
        "figure3a", "--json",
        "--omega-mhz", &omega, "--t-max-ns", &t_max, "--dt-ns", &dt, "--n-fock", &n_fock,
    ])?;
    Ok(Json(result))
}

/// POST /swap/figure3c — Accumulated SWAP error vs N gates.
/// Reproduces Fig. 3c of Mollenhauer et al.
async fn swap_figure3c(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_swaps = req.get("n_swaps").and_then(as_u64_loose).unwrap_or(40).to_string();
    let omega   = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let dt      = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let n_fock  = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let result = run_tool("rustyswap", &[
        "figure3c", "--json",
        "--n-swaps", &n_swaps, "--omega-mhz", &omega, "--dt-ns", &dt, "--n-fock", &n_fock,
    ])?;
    Ok(Json(result))
}

/// POST /swap/figure4c — Detuned Raman chevron pattern.
/// Reproduces Fig. 4c of Mollenhauer et al.
async fn swap_figure4c(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega     = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let big_delta = req.get("big_delta_mhz").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let d_min     = req.get("delta_min_mhz").and_then(|v| v.as_f64()).unwrap_or(-12.5);
    let d_max     = req.get("delta_max_mhz").and_then(|v| v.as_f64()).unwrap_or(12.5).to_string();
    let n_delta   = req.get("n_delta").and_then(as_u64_loose).unwrap_or(40).to_string();
    let t_max     = req.get("t_max_ns").and_then(|v| v.as_f64()).unwrap_or(2000.0).to_string();
    let dt        = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let n_fock    = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let d_min_str = format!("--delta-min-mhz={d_min}");
    let result = run_tool("rustyswap", &[
        "figure4c", "--json",
        "--omega-mhz", &omega, "--big-delta-mhz", &big_delta,
        &d_min_str, "--delta-max-mhz", &d_max,
        "--n-delta", &n_delta, "--t-max-ns", &t_max, "--dt-ns", &dt, "--n-fock", &n_fock,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Orchestrate handlers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// rustyscq handlers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// oqfp handlers
// ---------------------------------------------------------------------------

async fn oqfp_health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "oqfp" }))
}

/// POST /oqfp/validate — validate an OQFP spec (JSON body or file path)
/// Body: { "spec": <oqfp JSON object> }  OR  { "spec_path": "/path/to/file.oqfp.json" }
async fn oqfp_validate_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let (path, is_tmp) = write_oqfp_spec(&req)?;
    let result = run_tool("oqfp", &["validate", "--spec", &path, "--json"]);
    if is_tmp { let _ = std::fs::remove_file(&path); }
    Ok(Json(result?))
}

/// POST /oqfp/summary — human-readable summary of an OQFP spec
async fn oqfp_summary_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let (path, is_tmp) = write_oqfp_spec(&req)?;
    let result = run_tool("oqfp", &["summary", "--spec", &path, "--json"]);
    if is_tmp { let _ = std::fs::remove_file(&path); }
    Ok(Json(result?))
}

/// POST /oqfp/diff — diff two OQFP specs
/// Body: { "spec_a": <oqfp object>, "spec_b": <oqfp object> }
async fn oqfp_diff_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let pid = std::process::id();
    let (path_a, tmp_a) = if let Some(obj) = req.get("spec_a") {
        let p = format!("/tmp/oqfp_a_{pid}.json");
        std::fs::write(&p, serde_json::to_string(obj)?)?;
        (p, true)
    } else if let Some(p) = req.get("spec_a_path").and_then(|v| v.as_str()) {
        (p.to_string(), false)
    } else {
        return Err(anyhow::anyhow!("provide spec_a or spec_a_path").into());
    };
    let (path_b, tmp_b) = if let Some(obj) = req.get("spec_b") {
        let p = format!("/tmp/oqfp_b_{pid}.json");
        std::fs::write(&p, serde_json::to_string(obj)?)?;
        (p, true)
    } else if let Some(p) = req.get("spec_b_path").and_then(|v| v.as_str()) {
        (p.to_string(), false)
    } else {
        return Err(anyhow::anyhow!("provide spec_b or spec_b_path").into());
    };
    let result = run_tool("oqfp", &["diff", "--spec-a", &path_a, "--spec-b", &path_b, "--json"]);
    if tmp_a { let _ = std::fs::remove_file(&path_a); }
    if tmp_b { let _ = std::fs::remove_file(&path_b); }
    Ok(Json(result?))
}

/// POST /oqfp/create — create a template OQFP spec
/// Body: { "template": "sc_9q" | "sc_27q" | "sc_127q" | "spin_8q" | "ion_32q" | "atom_100q" }
async fn oqfp_create_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let template = req.get("template").and_then(|v| v.as_str()).unwrap_or("sc_9q");
    let pid = std::process::id();
    let outfile = format!("/tmp/oqfp_create_{pid}.json");
    // oqfp create writes to <template>.oqfp.json by default; use --output to control it
    let out = run_subprocess("oqfp", &["create", "--template", template, "--output", &outfile, "--json"])?;
    if out.status.success() {
        let content = std::fs::read_to_string(&outfile)
            .with_context(|| format!("oqfp create output not found at {outfile}"))?;
        let _ = std::fs::remove_file(&outfile);
        let spec: Value = serde_json::from_str(&content)
            .with_context(|| "oqfp create returned invalid JSON")?;
        Ok(Json(json!({ "template": template, "spec": spec })))
    } else {
        let _ = std::fs::remove_file(&outfile);
        Err(anyhow::anyhow!("oqfp create failed: {}", String::from_utf8_lossy(&out.stderr)).into())
    }
}

/// Helper: write spec JSON to a temp file, or return existing path.
fn write_oqfp_spec(req: &Value) -> anyhow::Result<(String, bool)> {
    let pid = std::process::id();
    if let Some(obj) = req.get("spec") {
        let p = format!("/tmp/oqfp_{pid}.json");
        std::fs::write(&p, serde_json::to_string(obj)?)?;
        Ok((p, true))
    } else if let Some(p) = req.get("spec_path").and_then(|v| v.as_str()) {
        Ok((p.to_string(), false))
    } else {
        anyhow::bail!("provide 'spec' (JSON object) or 'spec_path' (file path)");
    }
}

async fn scq_health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "rustyscq" }))
}

/// POST /scq/spectrum — transmon energy spectrum and f01/anharmonicity
async fn scq_spectrum(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ej     = req.get("ej").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ej"))?.to_string();
    let ec     = req.get("ec").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ec"))?.to_string();
    let levels = req.get("levels").and_then(as_u64_loose).unwrap_or(5).to_string();
    let ng     = req.get("ng").and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
    let result = run_tool("rustyscq", &[
        "--json", "spectrum",
        "--ej", &ej, "--ec", &ec, "--levels", &levels, "--ng", &ng,
    ])?;
    Ok(Json(result))
}

/// POST /scq/simulate — orchestrate-stage-friendly transmon device simulation.
///
/// Used by the `scq_simulate` orchestrate stage (`ScqSimulateStage` POSTs here
/// without ever being wired to /scq/spectrum — the only route that previously
/// matched). Accepts any of:
///   - `{ej, ec}` directly
///   - `{qubit_frequency_ghz, anharmonicity_mhz}` (derives ej, ec)
///   - any upstream `<dep>_output` carrying `best_candidate.predicted_hamiltonian`
///     (FlowDesignResponse shape from `/qpudidp/inverse-design-rmflow`)
async fn scq_simulate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let (ej, ec) = if let (Some(ej), Some(ec)) = (
        req.get("ej").and_then(|v| v.as_f64()),
        req.get("ec").and_then(|v| v.as_f64()),
    ) {
        (ej, ec)
    } else {
        // Derive from a Hamiltonian target via the centralized resolver, which
        // accepts all three field-name conventions at top level and in upstream
        // `<dep>_output.best_candidate.predicted_hamiltonian` (shared-IR audit).
        let (freq_ghz, anharm_mhz) = resolve_hamiltonian_target(&req);
        // Transmon limit: α ≈ -E_C ⇒ E_C ≈ |α|/1000 GHz; f ≈ √(8·E_J·E_C) − E_C
        let ec = (anharm_mhz.abs() / 1000.0).max(0.05);
        let ej = ((freq_ghz + ec).powi(2)) / (8.0 * ec);
        (ej, ec)
    };
    let levels = req.get("levels").and_then(as_u64_loose).unwrap_or(5).to_string();
    let ng = req.get("ng").and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
    let ej_s = ej.to_string();
    let ec_s = ec.to_string();
    let result = run_tool("rustyscq", &[
        "--json", "spectrum",
        "--ej", &ej_s, "--ec", &ec_s, "--levels", &levels, "--ng", &ng,
    ])?;
    Ok(Json(result))
}

/// POST /scq/dispersion — charge dispersion vs offset charge
async fn scq_dispersion(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ej        = req.get("ej").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ej"))?.to_string();
    let ec        = req.get("ec").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ec"))?.to_string();
    let ng_points = req.get("ng_points").and_then(as_u64_loose).unwrap_or(100).to_string();
    let result = run_tool("rustyscq", &[
        "--json", "dispersion",
        "--ej", &ej, "--ec", &ec, "--ng-points", &ng_points,
    ])?;
    Ok(Json(result))
}

/// POST /scq/flux-sweep — frequency vs flux for tunable transmon
async fn scq_flux_sweep(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ej_max      = req.get("ej_max").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ej_max"))?.to_string();
    let ec          = req.get("ec").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ec"))?.to_string();
    let d           = req.get("d").and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
    let flux_points = req.get("flux_points").and_then(as_u64_loose).unwrap_or(100).to_string();
    let result = run_tool("rustyscq", &[
        "--json", "flux-sweep",
        "--ej-max", &ej_max, "--ec", &ec, "--d", &d, "--flux-points", &flux_points,
    ])?;
    Ok(Json(result))
}

/// POST /scq/coherence — estimated T1/T2 coherence times
/// Optional phonon params: `phonon_alpha`, `phonon_cutoff_ghz`, `temp_mk` (default 20 mK)
async fn scq_coherence(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let ej       = req.get("ej").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ej"))?.to_string();
    let ec       = req.get("ec").and_then(|v| v.as_f64()).ok_or_else(|| anyhow::anyhow!("missing ec"))?.to_string();
    let material = req.get("material").and_then(|v| v.as_str()).unwrap_or("silicon");
    let q_factor = req.get("q_factor").and_then(as_u64_loose).unwrap_or(1_000_000).to_string();
    let mut result = run_tool("rustyscq", &[
        "--json", "coherence",
        "--ej", &ej, "--ec", &ec, "--material", material, "--q-factor", &q_factor,
    ])?;

    // Optional phonon bath contribution.
    if let (Some(alpha), Some(cutoff_ghz)) = (
        req.get("phonon_alpha").and_then(|v| v.as_f64()),
        req.get("phonon_cutoff_ghz").and_then(|v| v.as_f64()),
    ) {
        let temp_mk = req.get("temp_mk").and_then(|v| v.as_f64()).unwrap_or(20.0);
        // Approximate qubit frequency from ej/ec: f_01 ≈ sqrt(8*E_J*E_C) - E_C (GHz)
        let ej_ghz: f64 = ej.parse().unwrap_or(20.0);
        let ec_ghz: f64 = ec.parse().unwrap_or(0.3);
        let qubit_freq_ghz = (8.0 * ej_ghz * ec_ghz).sqrt() - ec_ghz;

        let phonon_t1_us = phonon_t1_us_inline(alpha, cutoff_ghz, temp_mk, qubit_freq_ghz);
        let phonon_t2_us = phonon_t2_us_inline(alpha, cutoff_ghz, temp_mk, qubit_freq_ghz);
        let phonon_dephasing_rate_mhz = phonon_dephasing_rate_mhz_inline(alpha, cutoff_ghz, temp_mk, qubit_freq_ghz);

        if let Some(obj) = result.as_object_mut() {
            obj.insert("phonon".to_owned(), serde_json::json!({
                "t1_us": phonon_t1_us,
                "t2_us": phonon_t2_us,
                "dephasing_rate_mhz": phonon_dephasing_rate_mhz,
                "alpha": alpha,
                "cutoff_ghz": cutoff_ghz,
                "temp_mk": temp_mk,
            }));
        }
    }

    Ok(Json(result))
}

fn phonon_dephasing_rate_mhz_inline(alpha: f64, cutoff_ghz: f64, temp_mk: f64, qubit_freq_ghz: f64) -> f64 {
    use std::f64::consts::PI;
    const HBAR: f64 = 1.054_571_817e-34;
    const KB: f64 = 1.380_649e-23;
    let temp_k = temp_mk * 1e-3;
    let omega_q = qubit_freq_ghz * 2.0 * PI * 1e9;
    let omega_c = cutoff_ghz * 2.0 * PI * 1e9;
    let gamma_rad_s = PI * alpha * KB * temp_k / HBAR;
    let correction = omega_q.powi(2) / (omega_q.powi(2) + omega_c.powi(2));
    gamma_rad_s * correction / (2.0 * PI) / 1e6
}

fn phonon_t1_us_inline(alpha: f64, cutoff_ghz: f64, temp_mk: f64, qubit_freq_ghz: f64) -> f64 {
    use std::f64::consts::PI;
    const HBAR: f64 = 1.054_571_817e-34;
    const KB: f64 = 1.380_649e-23;
    let temp_k = temp_mk * 1e-3;
    let omega_q = qubit_freq_ghz * 2.0 * PI * 1e9;
    let omega_c = cutoff_ghz * 2.0 * PI * 1e9;
    let j_omega = alpha * omega_q * (-omega_q / omega_c).exp();
    let x = HBAR * omega_q / (2.0 * KB * temp_k);
    let coth = if x > 50.0 { 1.0 } else { x.cosh() / x.sinh() };
    let gamma_1 = j_omega * coth;
    (1.0 / gamma_1) * 1e6
}

fn phonon_t2_us_inline(alpha: f64, cutoff_ghz: f64, temp_mk: f64, qubit_freq_ghz: f64) -> f64 {
    let t1 = phonon_t1_us_inline(alpha, cutoff_ghz, temp_mk, qubit_freq_ghz);
    let gamma_phi = phonon_dephasing_rate_mhz_inline(alpha, cutoff_ghz, temp_mk, qubit_freq_ghz);
    let gamma_2 = (1.0 / t1) / 2.0 + gamma_phi;
    1.0 / gamma_2
}

// ---------------------------------------------------------------------------
// qorchestrate proxy (HTTP → qorchestrate :8767)
// ---------------------------------------------------------------------------

const QORCH_URL: &str = "http://127.0.0.1:8767";

fn qorch_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .unwrap_or_default()
}

async fn qorch_proxy_post(path: &str, body: Value) -> ApiResult<Json<Value>> {
    let url = format!("{QORCH_URL}{path}");
    let resp = qorch_client()
        .post(&url)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("failed to reach qorchestrate at {url}"))?;
    let status = resp.status();
    let text = resp.text().await.context("failed to read qorchestrate response")?;
    if !status.is_success() {
        return Err(anyhow::anyhow!("qorchestrate {path} returned HTTP {status}: {text}").into());
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON from qorchestrate: {text}"))?;
    Ok(Json(value))
}

async fn qorch_proxy_get(path: &str) -> ApiResult<Json<Value>> {
    let url = format!("{QORCH_URL}{path}");
    let resp = qorch_client()
        .get(&url)
        .send()
        .await
        .with_context(|| format!("failed to reach qorchestrate at {url}"))?;
    let status = resp.status();
    let text = resp.text().await.context("failed to read qorchestrate response")?;
    if !status.is_success() {
        return Err(anyhow::anyhow!("qorchestrate {path} returned HTTP {status}: {text}").into());
    }
    let value: Value = serde_json::from_str(&text)
        .with_context(|| format!("invalid JSON from qorchestrate: {text}"))?;
    Ok(Json(value))
}

/// GET /orchestrate/health
async fn orchestrate_health() -> Json<Value> {
    Json(serde_json::json!({ "status": "ok", "service": "qorchestrate", "port": 8767 }))
}

/// GET /orchestrate/stages — list pipeline templates
async fn orchestrate_stages() -> ApiResult<Json<Value>> {
    qorch_proxy_get("/pipeline/templates").await
}

/// POST /orchestrate/validate — validate a pipeline template or inline TOML
async fn orchestrate_validate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qorch_proxy_post("/pipeline/validate", req).await
}

/// POST /orchestrate/run — run a pipeline
/// Body: { "template": "chip_design", "params": {} }  OR  { "pipeline_toml": "..." }
async fn orchestrate_run(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    qorch_proxy_post("/pipeline/run", req).await
}

/// POST /swap/fock-convergence — Fock truncation convergence study.
async fn swap_fock_convergence(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_fock_min = req.get("n_fock_min").and_then(as_u64_loose).unwrap_or(2).to_string();
    let n_fock_max = req.get("n_fock_max").and_then(as_u64_loose).unwrap_or(6).to_string();
    let omega      = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let t_max      = req.get("t_max_ns").and_then(|v| v.as_f64()).unwrap_or(400.0).to_string();
    let dt         = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let result = run_tool("rustyswap", &[
        "fock-convergence", "--json",
        "--n-fock-min", &n_fock_min, "--n-fock-max", &n_fock_max,
        "--omega-mhz", &omega, "--t-max-ns", &t_max, "--dt-ns", &dt,
    ])?;
    Ok(Json(result))
}

/// POST /swap/sw-validity — Schrieffer-Wolff approximation validity sweep.
async fn swap_sw_validity(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega      = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let ratio_min  = req.get("delta_ratio_min").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let ratio_max  = req.get("delta_ratio_max").and_then(|v| v.as_f64()).unwrap_or(20.0).to_string();
    let n_delta    = req.get("n_delta").and_then(as_u64_loose).unwrap_or(30).to_string();
    let dt         = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(0.5).to_string();
    let ratio_min_str = format!("--delta-ratio-min={ratio_min}");
    let result = run_tool("rustyswap", &[
        "sw-validity", "--json",
        "--omega-mhz", &omega, &ratio_min_str,
        "--delta-ratio-max", &ratio_max, "--n-delta", &n_delta, "--dt-ns", &dt,
    ])?;
    Ok(Json(result))
}

/// POST /swap/nmodule-chain — 3-module chain SWAP simulation.
async fn swap_nmodule_chain(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let routing    = req.get("routing").and_then(|v| v.as_str()).unwrap_or("sequential").to_string();
    let omega_12   = req.get("omega_12_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let omega_23   = req.get("omega_23_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let tau_b      = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1         = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2         = req.get("t2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let n_fock     = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let t_max      = req.get("t_max_ns").and_then(|v| v.as_f64()).unwrap_or(600.0).to_string();
    let dt         = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let result = run_tool("rustyswap", &[
        "nmodule-chain", "--json",
        "--routing", &routing, "--omega-12-mhz", &omega_12, "--omega-23-mhz", &omega_23,
        "--tau-b", &tau_b, "--t1", &t1, "--t2", &t2, "--n-fock", &n_fock,
        "--t-max-ns", &t_max, "--dt-ns", &dt,
    ])?;
    Ok(Json(result))
}

/// POST /swap/tls-loss — Power-dependent TLS loss model sweep.
async fn swap_tls_loss(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let alpha       = req.get("alpha").and_then(|v| v.as_f64()).unwrap_or(0.01).to_string();
    let beta        = req.get("beta").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let onset       = req.get("onset_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let omega_min   = req.get("omega_min_mhz").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let omega_max   = req.get("omega_max_mhz").and_then(|v| v.as_f64()).unwrap_or(12.0).to_string();
    let n_omega     = req.get("n_omega").and_then(as_u64_loose).unwrap_or(20).to_string();
    let dt          = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let result = run_tool("rustyswap", &[
        "tls-loss", "--json",
        "--alpha", &alpha, "--beta", &beta, "--onset-mhz", &onset,
        "--omega-min-mhz", &omega_min, "--omega-max-mhz", &omega_max,
        "--n-omega", &n_omega, "--dt-ns", &dt,
    ])?;
    Ok(Json(result))
}

/// POST /swap/chi-sensitivity — χ dispersive coupling sensitivity to cable placement.
async fn swap_chi_sensitivity(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let d_nom       = req.get("d_nominal_mm").and_then(|v| v.as_f64()).unwrap_or(0.764).to_string();
    let sigma_d     = req.get("sigma_d_mm").and_then(|v| v.as_f64()).unwrap_or(0.167).to_string();
    let n_samples   = req.get("n_samples").and_then(as_u64_loose).unwrap_or(10000).to_string();
    let q_freq      = req.get("qubit_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.2).to_string();
    let c_freq      = req.get("cavity_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(7.7).to_string();
    let q_cap       = req.get("qubit_cap_ff").and_then(|v| v.as_f64()).unwrap_or(105.0).to_string();
    let r_cable     = req.get("cable_radius_mm").and_then(|v| v.as_f64()).unwrap_or(0.255).to_string();
    let d_min       = req.get("d_min_mm").and_then(|v| v.as_f64()).unwrap_or(0.1).to_string();
    let d_max       = req.get("d_max_mm").and_then(|v| v.as_f64()).unwrap_or(3.0).to_string();
    let n_sweep     = req.get("n_sweep").and_then(as_u64_loose).unwrap_or(50).to_string();
    let result = run_tool("rustyswap", &[
        "chi-sensitivity", "--json",
        "--d-nominal-mm", &d_nom, "--sigma-d-mm", &sigma_d,
        "--n-samples", &n_samples,
        "--qubit-freq-ghz", &q_freq, "--cavity-freq-ghz", &c_freq,
        "--qubit-cap-ff", &q_cap, "--cable-radius-mm", &r_cable,
        "--d-min-mm", &d_min, "--d-max-mm", &d_max, "--n-sweep", &n_sweep,
    ])?;
    Ok(Json(result))
}

/// POST /swap/spam-model — SPAM readout error model applied to Lindblad SWAP fidelity.
///
/// Reports raw / measured / SPAM-corrected fidelity and the error budget.
async fn swap_spam_model(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega    = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let tau_b    = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1_a1    = req.get("t1_a1_us").and_then(|v| v.as_f64()).unwrap_or(62.0).to_string();
    let t2_a1    = req.get("t2_a1_us").and_then(|v| v.as_f64()).unwrap_or(22.0).to_string();
    let t1_a2    = req.get("t1_a2_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2_a2    = req.get("t2_a2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let e10_q1   = req.get("e10_q1").and_then(|v| v.as_f64()).unwrap_or(0.010).to_string();
    let e01_q1   = req.get("e01_q1").and_then(|v| v.as_f64()).unwrap_or(0.005).to_string();
    let e10_q2   = req.get("e10_q2").and_then(|v| v.as_f64()).unwrap_or(0.010).to_string();
    let e01_q2   = req.get("e01_q2").and_then(|v| v.as_f64()).unwrap_or(0.005).to_string();
    let result = run_tool("rustyswap", &[
        "spam-model", "--json",
        "--omega-mhz", &omega, "--tau-b", &tau_b,
        "--t1-a1", &t1_a1, "--t2-a1", &t2_a1,
        "--t1-a2", &t1_a2, "--t2-a2", &t2_a2,
        "--e10-q1", &e10_q1, "--e01-q1", &e01_q1,
        "--e10-q2", &e10_q2, "--e01-q2", &e01_q2,
    ])?;
    Ok(Json(result))
}

/// POST /swap/crosstalk-sweep — ZZ crosstalk sweep for 3-module chain.
async fn swap_crosstalk_sweep(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega       = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let tau_b       = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1          = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2          = req.get("t2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let n_fock      = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let xi_max      = req.get("xi_max_mhz").and_then(|v| v.as_f64()).unwrap_or(0.5).to_string();
    let n_xi        = req.get("n_xi").and_then(as_u64_loose).unwrap_or(10).to_string();
    let dt_ns       = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let qubit_freq  = req.get("qubit_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.2).to_string();
    let result = run_tool("rustyswap", &[
        "crosstalk-sweep", "--json",
        "--omega-mhz", &omega, "--tau-b", &tau_b,
        "--t1", &t1, "--t2", &t2, "--n-fock", &n_fock,
        "--xi-max-mhz", &xi_max, "--n-xi", &n_xi,
        "--dt-ns", &dt_ns, "--qubit-freq-ghz", &qubit_freq,
    ])?;
    Ok(Json(result))
}

/// POST /swap/param-spread — Monte Carlo coupling spread for 3-module chain.
async fn swap_param_spread(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega       = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let spread      = req.get("spread_sigma").and_then(|v| v.as_f64()).unwrap_or(0.22).to_string();
    let tau_b       = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1          = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2          = req.get("t2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let n_fock      = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let n_samples   = req.get("n_samples").and_then(as_u64_loose).unwrap_or(50).to_string();
    let dt_ns       = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let result = run_tool("rustyswap", &[
        "param-spread", "--json",
        "--omega-mhz", &omega, "--spread-sigma", &spread,
        "--tau-b", &tau_b, "--t1", &t1, "--t2", &t2,
        "--n-fock", &n_fock, "--n-samples", &n_samples, "--dt-ns", &dt_ns,
    ])?;
    Ok(Json(result))
}

/// POST /swap/nmodule-scaling — sweep N=n_min..n_max sequential SWAP fidelity.
async fn swap_nmodule_scaling(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_min   = req.get("n_min").and_then(as_u64_loose).unwrap_or(2).to_string();
    let n_max   = req.get("n_max").and_then(as_u64_loose).unwrap_or(4).to_string();
    let omega   = req.get("omega_mhz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let tau_b   = req.get("tau_b_us").and_then(|v| v.as_f64()).unwrap_or(6.2).to_string();
    let t1      = req.get("t1_us").and_then(|v| v.as_f64()).unwrap_or(25.0).to_string();
    let t2      = req.get("t2_us").and_then(|v| v.as_f64()).unwrap_or(8.0).to_string();
    let n_fock  = req.get("n_fock").and_then(as_u64_loose).unwrap_or(4).to_string();
    let dt_ns   = req.get("dt_ns").and_then(|v| v.as_f64()).unwrap_or(2.0).to_string();
    let result = run_tool("rustyswap", &[
        "n-module-scaling", "--json",
        "--n-min", &n_min, "--n-max", &n_max,
        "--omega-mhz", &omega, "--tau-b", &tau_b,
        "--t1", &t1, "--t2", &t2, "--n-fock", &n_fock, "--dt-ns", &dt_ns,
    ])?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// Phase 7X: claw-mesh handlers
// ---------------------------------------------------------------------------

fn pcell_to_json(cell: &PCell) -> Value {
    let polygons: Vec<Value> = cell.polygons.iter().map(|p| json!({
        "layer": p.layer.layer,
        "datatype": p.layer.datatype,
        "vertices": p.vertices,
    })).collect();
    let ports: Vec<Value> = cell.ports.iter().map(|p| json!({
        "name": p.name,
        "position": p.position,
        "direction": p.direction,
        "width": p.width,
    })).collect();
    json!({
        "name": cell.name,
        "n_polygons": cell.polygons.len(),
        "n_ports": cell.ports.len(),
        "bbox_um": cell.bbox(),
        "polygons": polygons,
        "ports": ports,
    })
}

async fn mesh_health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "claw-mesh"}))
}

async fn mesh_transmon_cross(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: TransmonCrossParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, ja, jb) = build_transmon_cross_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "junction_a_um": ja,
        "junction_b_um": jb,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_rectangular_cavity_3d(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: RectangularCavity3DParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, sma_ports) = build_rectangular_cavity_3d_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "n_sma_ports": sma_ports.len(),
        "sma_ports": serde_json::to_value(&sma_ports).unwrap_or_default(),
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_tunable_transmon(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: TunableTransmonMeshParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, ja, jb) = build_tunable_transmon_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "junction_a_um": ja,
        "junction_b_um": jb,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_xmon(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: XmonMeshParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, ja, jb) = build_xmon_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "junction_a_um": ja,
        "junction_b_um": jb,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_fluxonium(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: FluxoniumMeshParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, ja, jb) = build_fluxonium_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "junction_a_um": ja,
        "junction_b_um": jb,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_cpw_resonator(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: CpwResonatorMeshParams = serde_json::from_value(req)
        .unwrap_or_default();
    let (mesh, port_a, port_b) = build_cpw_resonator_mesh(&params)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&mesh);
    Ok(Json(json!({
        "n_vertices": mesh.num_vertices(),
        "n_tetrahedra": mesh.num_tetrahedra(),
        "total_volume_um3": mesh.total_volume(),
        "port_a_um": port_a,
        "port_b_um": port_b,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&mesh).unwrap_or_default(),
    })))
}

async fn mesh_chip(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config: ChipMeshConfig = serde_json::from_value(req)
        .unwrap_or_default();
    let result = build_chip_mesh(&config)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let quality = mesh_quality(&result.mesh);
    Ok(Json(json!({
        "n_vertices": result.mesh.num_vertices(),
        "n_tetrahedra": result.mesh.num_tetrahedra(),
        "total_volume_um3": result.mesh.total_volume(),
        "num_qubits": result.num_qubits,
        "num_couplers": result.num_couplers,
        "region_labels": result.region_labels,
        "quality": serde_json::to_value(&quality).unwrap_or_default(),
        "mesh": serde_json::to_value(&result.mesh).unwrap_or_default(),
    })))
}

async fn mesh_quality_endpoint(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let mesh: ClawTetMesh = serde_json::from_value(
        req.get("mesh").cloned().unwrap_or(req.clone())
    ).map_err(|e| anyhow::anyhow!("mesh parse error: {e}"))?;
    let report = mesh_quality(&mesh);
    Ok(Json(serde_json::to_value(&report).map_err(|e| anyhow::anyhow!("{e}"))?))
}

// ---------------------------------------------------------------------------
// Phase 7X: claw-gds handlers
// ---------------------------------------------------------------------------

async fn gds_health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "claw-gds"}))
}

async fn clawprint_health() -> Json<Value> {
    Json(json!({"status": "ok", "service": "clawprint"}))
}

/// POST /clawprint/dressed — transmon⊗resonator dressed analysis (χ, e0g1/f0g1
/// sideband frequencies, dressed spectrum) via the `clawprint` CLI. Body fields
/// are all optional (the CLI supplies defaults): {f01_ghz, ec_ghz, ng, n_cut,
/// transmon_dim, omega_r_ghz, res_ec_ghz, fock_dim, g_ghz, target_chi_mhz,
/// n_report}. Omit `g_ghz` to calibrate the coupling to `target_chi_mhz`.
async fn clawprint_dressed(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let mut argv: Vec<String> = vec!["dressed".to_string()];
    for (key, flag) in [
        ("f01_ghz", "--f01-ghz"),
        ("ec_ghz", "--ec-ghz"),
        ("ng", "--ng"),
        ("omega_r_ghz", "--omega-r-ghz"),
        ("res_ec_ghz", "--res-ec-ghz"),
        ("g_ghz", "--g-ghz"),
        ("target_chi_mhz", "--target-chi-mhz"),
    ] {
        if let Some(x) = req.get(key).and_then(|v| v.as_f64()) {
            argv.push(flag.to_string());
            argv.push(format!("{x}"));
        }
    }
    for (key, flag) in [
        ("n_cut", "--n-cut"),
        ("transmon_dim", "--transmon-dim"),
        ("fock_dim", "--fock-dim"),
        ("n_report", "--n-report"),
    ] {
        if let Some(x) = req.get(key).and_then(|v| v.as_u64()) {
            argv.push(flag.to_string());
            argv.push(x.to_string());
        }
    }
    let args: Vec<&str> = argv.iter().map(String::as_str).collect();
    let result = run_tool("clawprint", &args)?;
    Ok(Json(result))
}

async fn gds_transmon_cross(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: TransmonCrossParams = serde_json::from_value(req)
        .unwrap_or_default();
    let cell = build_transmon_cross_pcell(&params);
    Ok(Json(pcell_to_json(&cell)))
}

async fn gds_rectangular_cavity_3d(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: RectangularCavity3DParams = serde_json::from_value(req)
        .unwrap_or_default();
    let cell = build_rectangular_cavity_3d_pcell(&params);
    Ok(Json(pcell_to_json(&cell)))
}

/// Build a `ChipConfig` from a loose JSON request body. Shared by the
/// chip-layout, chip GDS export, and DRC handlers so they all interpret the
/// same `{cols, rows, pitch_x, pitch_y, qubit_params}` shape identically.
fn chip_config_from_value(req: &Value) -> ChipConfig {
    let mut config = ChipConfig::default();
    if let Some(cols) = req.get("cols").and_then(as_u64_loose) { config.cols = cols as usize; }
    if let Some(rows) = req.get("rows").and_then(as_u64_loose) { config.rows = rows as usize; }
    if let Some(px) = req.get("pitch_x").and_then(|v| v.as_f64()) { config.pitch_x = px; }
    if let Some(py) = req.get("pitch_y").and_then(|v| v.as_f64()) { config.pitch_y = py; }
    if let Some(qp) = req.get("qubit_params") && let Ok(p) = serde_json::from_value::<TransmonCrossParams>(qp.clone()) {
        config.qubit_params = p;
    }
    config
}

async fn gds_chip_layout(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config = chip_config_from_value(&req);
    let layout = build_chip_layout(&config);
    let cell_json = pcell_to_json(&layout.cell);
    Ok(Json(json!({
        "num_qubits": layout.num_qubits,
        "num_resonators": layout.num_resonators,
        "num_bus_couplers": layout.num_bus_couplers,
        "layout": cell_json,
    })))
}

/// Generate a full multi-qubit chip layout and export it as GDS-II bytes
/// (hex-encoded). This is the chip-level analogue of `/gds/export`, which only
/// exports a single transmon. Used by the orchestration `gds_generate` stage to
/// produce a fabrication-ready artifact for the whole design.
/// JSON mask table for a layer map (the tape-out deck).
fn layer_map_table(map: &LayerMap) -> Value {
    let layers: Vec<Value> = map.entries.iter().map(|e| json!({
        "source_layer": e.source.layer,
        "source_datatype": e.source.datatype,
        "gds_layer": e.target.layer,
        "gds_datatype": e.target.datatype,
        "mask_name": e.mask_name,
        "polarity": e.polarity,
    })).collect();
    json!({ "name": map.name, "layers": layers })
}

/// Shoelace area of a polygon (µm²).
fn polygon_area_um2(verts: &[[f64; 2]]) -> f64 {
    let n = verts.len();
    if n < 3 {
        return 0.0;
    }
    let mut a = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        a += verts[i][0] * verts[j][1] - verts[j][0] * verts[i][1];
    }
    a.abs() * 0.5
}

/// Mask job deck: per-mask-layer summary of the final (remapped) layout — what a
/// foundry / mask shop needs to plan the mask set.
fn mask_job_deck(cell: &PCell, map: &LayerMap) -> Value {
    use std::collections::BTreeMap;
    // target GDS layer → (mask_name, polarity)
    let mut names: BTreeMap<(i16, i16), (&str, &str)> = BTreeMap::new();
    for e in map.entries {
        names.insert((e.target.layer, e.target.datatype), (e.mask_name, e.polarity));
    }
    // group polygons by GDS layer: (count, bbox, total area)
    let mut groups: BTreeMap<(i16, i16), (usize, [f64; 4], f64)> = BTreeMap::new();
    for p in &cell.polygons {
        let g = groups.entry((p.layer.layer, p.layer.datatype)).or_insert((
            0,
            [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY],
            0.0,
        ));
        g.0 += 1;
        for &[x, y] in &p.vertices {
            g.1[0] = g.1[0].min(x);
            g.1[1] = g.1[1].min(y);
            g.1[2] = g.1[2].max(x);
            g.1[3] = g.1[3].max(y);
        }
        g.2 += polygon_area_um2(&p.vertices);
    }
    let masks: Vec<Value> = groups
        .iter()
        .map(|(&(l, d), &(n, bb, area))| {
            let (mn, pol) = names.get(&(l, d)).copied().unwrap_or(("unknown", "positive"));
            json!({
                "gds_layer": l, "gds_datatype": d, "mask_name": mn, "polarity": pol,
                "n_polygons": n, "bbox_um": bb, "area_um2": area,
            })
        })
        .collect();
    json!({ "n_masks": masks.len(), "masks": masks })
}

async fn gds_export_chip(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config = chip_config_from_value(&req);
    let layout = build_chip_layout(&config);

    // Optional foundry layer map (tape-out deck): remap logical layers to the
    // foundry's mask layers. Unknown name → "default" identity map.
    let requested = req
        .get("layer_map")
        .or_else(|| req.get("deck"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let (map_name, map) = match layer_mapping(&requested) {
        Some(m) => (requested, m),
        None => ("default".to_string(), layer_mapping("default").unwrap()),
    };

    // Tape-out fab-prep: dummy fill first (inside the device), then the frame
    // (alignment marks + dicing lanes, outside).
    let mut cell = layout.cell.clone();
    let n_fill = if req.get("dummy_fill").and_then(|v| v.as_bool()).unwrap_or(false) {
        Some(add_dummy_fill(&mut cell, &FillOptions::default()))
    } else {
        None
    };
    let frame = if req.get("tapeout_frame").and_then(|v| v.as_bool()).unwrap_or(false) {
        Some(add_tapeout_frame(&mut cell, &FrameOptions::default()))
    } else {
        None
    };
    let cell = remap_cell(&cell, &map);
    let job_deck = mask_job_deck(&cell, &map);
    // Mask fracture: decompose every figure into trapezoids + compliance report.
    let vertex_limit = req
        .get("vertex_limit")
        .and_then(as_u64_loose)
        .unwrap_or(200) as usize;
    let fracture = claw_gds::fracture::fracture_cell(&cell, vertex_limit);

    let tmp = NamedTempFile::new().context("tempfile")?;
    let path = tmp.path().with_extension("gds");
    write_gds(&path, "quantum_api_chip", &[&cell])
        .map_err(|e| anyhow::anyhow!("GDS write: {e}"))?;
    let bytes = std::fs::read(&path).context("read GDS")?;
    let hex = bytes.iter().fold(String::new(), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    });
    Ok(Json(json!({
        "format": "gds2",
        "lib_name": "quantum_api_chip",
        "n_bytes": bytes.len(),
        "hex": hex,
        "num_qubits": layout.num_qubits,
        "num_resonators": layout.num_resonators,
        "num_bus_couplers": layout.num_bus_couplers,
        "layer_map": map_name,
        "layer_table": layer_map_table(&map),
        "tapeout_frame": frame.map(|f| json!({
            "alignment_marks": f.n_alignment_marks,
            "dicing_outer_um": f.dicing_outer_um,
        })),
        "dummy_fill_tiles": n_fill,
        "job_deck": job_deck,
        "fracture": serde_json::to_value(&fracture).unwrap_or(Value::Null),
    })))
}

/// List the available foundry layer maps (tape-out decks) and their mask tables.
async fn tapeout_layermaps() -> Json<Value> {
    let maps: Vec<Value> = layer_mapping_names()
        .iter()
        .filter_map(|n| layer_mapping(n))
        .map(|m| layer_map_table(&m))
        .collect();
    Json(json!({ "layer_maps": maps }))
}

/// Serialize a DRC rule deck's per-layer rules for the `/drc/decks` endpoint.
fn drc_config_to_json(config: &DrcConfig) -> Value {
    let rules: Vec<Value> = config.layer_rules.iter().map(|r| json!({
        "layer": r.layer.layer,
        "datatype": r.layer.datatype,
        "min_width_um": r.min_width,
        "min_spacing_um": r.min_spacing,
        "check_width": r.check_width,
        "check_spacing": r.check_spacing,
        "check_overlap": r.check_overlap,
    })).collect();
    json!({ "layer_rules": rules })
}

/// Run a design-rule check over a generated chip layout. The request body is
/// the same `{cols, rows, pitch_x, pitch_y, qubit_params}` chip-layout shape,
/// plus an optional `pdk` (alias `deck`) selecting a named foundry rule deck
/// (default `"default"`). The layout is rebuilt deterministically and checked,
/// returning the full violation list, a `clean` flag, and the deck used.
async fn gds_drc(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let config = chip_config_from_value(&req);
    let layout = build_chip_layout(&config);
    let requested = req
        .get("pdk")
        .or_else(|| req.get("deck"))
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    // Fall back to the default deck for an unknown name, but report which deck
    // actually ran so the caller is never misled.
    let (deck, drc_config) = match DrcConfig::deck(&requested) {
        Some(c) => (requested, c),
        None => ("default".to_string(), DrcConfig::default()),
    };
    let violations = check_drc(&layout.cell, &drc_config);
    let violations_json: Vec<Value> = violations.iter().map(|v| json!({
        "rule": format!("{:?}", v.rule),
        "message": v.message,
        "location": v.location,
    })).collect();
    Ok(Json(json!({
        "deck": deck,
        "clean": violations.is_empty(),
        "num_violations": violations.len(),
        "violations": violations_json,
    })))
}

/// List the available named DRC rule decks and their per-layer rules.
async fn gds_drc_decks() -> Json<Value> {
    let decks: Vec<Value> = DrcConfig::deck_names().iter().filter_map(|name| {
        DrcConfig::deck(name).map(|cfg| {
            let mut d = drc_config_to_json(&cfg);
            d["name"] = json!(name);
            d
        })
    }).collect();
    Json(json!({ "decks": decks }))
}

/// List the available foundry submission profiles (PDK deck + process + test
/// plan + DRC gating) used by the tape-out package assembler.
async fn foundry_profiles() -> Json<Value> {
    Json(qservices_common::foundry::all_profiles_json())
}

// ---------------------------------------------------------------------------
// Josephson-junction process/recipe model (claw-yield)
// ---------------------------------------------------------------------------

/// Resolve a `JunctionRecipe` from a request body: either `{"recipe":"<name>"}`
/// (named) or an inline recipe object.
fn junction_recipe_from_value(req: &Value) -> anyhow::Result<JunctionRecipe> {
    if let Some(name) = req.get("recipe").and_then(|v| v.as_str()) {
        jj_recipe(name).ok_or_else(|| anyhow::anyhow!("unknown junction recipe '{name}'"))
    } else {
        serde_json::from_value(req.clone()).map_err(|e| anyhow::anyhow!("invalid recipe: {e}"))
    }
}

fn recipe_json(r: &JunctionRecipe) -> Value {
    json!({
        "recipe": serde_json::to_value(r).unwrap_or(Value::Null),
        "eval": serde_json::to_value(r.evaluate()).unwrap_or(Value::Null),
        "process_params": serde_json::to_value(r.to_process_params()).unwrap_or(Value::Null),
    })
}

/// List the built-in junction recipes with their evaluated nominal (E_J/L_J/I_c)
/// and derived process parameters.
async fn junction_recipes() -> Json<Value> {
    let recipes: Vec<Value> = jj_recipe_names()
        .iter()
        .filter_map(|n| jj_recipe(n))
        .map(|r| recipe_json(&r))
        .collect();
    Json(json!({ "recipes": recipes }))
}

/// Evaluate a junction recipe → nominal I_c / E_J / L_J + ProcessParams.
async fn junction_recipe_eval(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let r = junction_recipe_from_value(&req)?;
    Ok(Json(recipe_json(&r)))
}

fn design_from_req(req: &Value) -> anyhow::Result<NominalDesign> {
    let d = req
        .get("design")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing 'design'"))?;
    serde_json::from_value(d).map_err(|e| anyhow::anyhow!("invalid design: {e}"))
}

/// Monte-Carlo yield for a design under a recipe (or explicit ProcessParams),
/// GPU-accelerated when available. Returns the backend that ran it.
async fn junction_yield(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let design = design_from_req(&req)?;
    let samples = req.get("samples").and_then(as_u64_loose).unwrap_or(50_000) as usize;
    let threshold = req
        .get("collision_threshold_mhz")
        .and_then(|v| v.as_f64())
        .unwrap_or(40.0);
    let process: ProcessParams = if req.get("recipe").is_some() {
        junction_recipe_from_value(&req)?.to_process_params()
    } else {
        serde_json::from_value(
            req.get("process")
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing 'process' or 'recipe'"))?,
        )
        .map_err(|e| anyhow::anyhow!("invalid process: {e}"))?
    };
    let a = claw_yield_gpu::monte_carlo_yield_auto(&design, &process, samples, threshold);
    Ok(Json(json!({
        "backend": a.backend,
        "yield": serde_json::to_value(&a.result).unwrap_or(Value::Null),
    })))
}

/// Reverse tolerance budget: largest junction σ(%) meeting a target yield, with
/// the dominant variance contributor. GPU-accelerated MC when available.
async fn junction_budget(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let design = design_from_req(&req)?;
    let recipe = junction_recipe_from_value(&req)?;
    let target = req.get("target_yield").and_then(|v| v.as_f64()).unwrap_or(0.9);
    let samples = req.get("samples").and_then(as_u64_loose).unwrap_or(50_000) as usize;
    let threshold = req
        .get("collision_threshold_mhz")
        .and_then(|v| v.as_f64())
        .unwrap_or(40.0);

    let backend = std::cell::Cell::new("cpu");
    let budget = tolerance_budget_with(
        &design,
        &recipe,
        target,
        samples,
        threshold,
        |d, p, s, t| {
            let a = claw_yield_gpu::monte_carlo_yield_auto(d, p, s, t);
            backend.set(a.backend);
            a.result
        },
    );
    Ok(Json(json!({
        "backend": backend.get(),
        "budget": serde_json::to_value(&budget).unwrap_or(Value::Null),
        "recipe": serde_json::to_value(&recipe).unwrap_or(Value::Null),
        "eval": serde_json::to_value(recipe.evaluate()).unwrap_or(Value::Null),
    })))
}

async fn gds_export(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let params: TransmonCrossParams = serde_json::from_value(
        req.get("params").cloned().unwrap_or_default()
    ).unwrap_or_default();
    let cell = build_transmon_cross_pcell(&params);
    let tmp = NamedTempFile::new().context("tempfile")?;
    let path = tmp.path().with_extension("gds");
    write_gds(&path, "quantum_api", &[&cell])
        .map_err(|e| anyhow::anyhow!("GDS write: {e}"))?;
    let bytes = std::fs::read(&path).context("read GDS")?;
    let b64 = bytes.iter().fold(String::new(), |mut s, b| {
        s.push_str(&format!("{b:02x}"));
        s
    });
    Ok(Json(json!({
        "format": "gds2",
        "lib_name": "quantum_api",
        "n_bytes": bytes.len(),
        "hex": b64,
    })))
}

// ---------------------------------------------------------------------------
// Phase 7Y: clawview proxy handlers
// ---------------------------------------------------------------------------

const CLAWVIEW_URL: &str = "http://127.0.0.1:9090";

async fn clawview_health() -> Json<Value> {
    match reqwest::get(format!("{CLAWVIEW_URL}/api/formats")).await {
        Ok(r) if r.status().is_success() => Json(json!({"status": "ok", "port": 9090})),
        _ => Json(json!({"status": "unavailable", "port": 9090})),
    }
}

async fn clawview_participation() -> ApiResult<Json<Value>> {
    let resp = reqwest::get(format!("{CLAWVIEW_URL}/api/participation"))
        .await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_streamlines(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let resp = reqwest::Client::new()
        .post(format!("{CLAWVIEW_URL}/api/streamlines"))
        .json(&req).send().await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_isosurface() -> ApiResult<Json<Value>> {
    let resp = reqwest::get(format!("{CLAWVIEW_URL}/api/isosurface"))
        .await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_coupling() -> ApiResult<Json<Value>> {
    let resp = reqwest::get(format!("{CLAWVIEW_URL}/api/coupling"))
        .await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_surrogate_predict(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let resp = reqwest::Client::new()
        .post(format!("{CLAWVIEW_URL}/api/surrogate/predict"))
        .json(&req).send().await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_cross_section(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let resp = reqwest::Client::new()
        .post(format!("{CLAWVIEW_URL}/api/cross-section"))
        .json(&req).send().await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_layout_from_params(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let resp = reqwest::Client::new()
        .post(format!("{CLAWVIEW_URL}/api/layout/from_params"))
        .json(&req).send().await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

async fn clawview_formats() -> ApiResult<Json<Value>> {
    let resp = reqwest::get(format!("{CLAWVIEW_URL}/api/formats"))
        .await.context("clawview request")?
        .json::<Value>().await.context("clawview json")?;
    Ok(Json(resp))
}

// ---------------------------------------------------------------------------
// Phase 8K–8R handlers
// ---------------------------------------------------------------------------

/// POST /floquet/grape-su2 — GRAPE optimal control targeting SU(2) with leakage penalty.
async fn floquet_grape_su2(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_timesteps = req.get("n_timesteps").and_then(as_u64_loose).unwrap_or(50);
    let total_time_ns = req.get("total_time_ns").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let anharmonicity_mhz = req.get("anharmonicity_mhz").and_then(|v| v.as_f64()).unwrap_or(-300.0);
    let leakage_weight = req.get("leakage_weight").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let max_amplitude_mhz = req.get("max_amplitude_mhz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let n_iterations = req.get("n_iterations").and_then(as_u64_loose).unwrap_or(200);
    let arg_n_timesteps = format!("--n-timesteps={n_timesteps}");
    let arg_total_time_ns = format!("--total-time-ns={total_time_ns}");
    let arg_anharmonicity_mhz = format!("--anharmonicity-mhz={anharmonicity_mhz}");
    let arg_leakage_weight = format!("--leakage-weight={leakage_weight}");
    let arg_max_amplitude_mhz = format!("--max-amplitude-mhz={max_amplitude_mhz}");
    let arg_n_iterations = format!("--n-iterations={n_iterations}");
    let result = run_tool("rustyfloquet", &[
        "grape-su2", "--json",
        &arg_n_timesteps, &arg_total_time_ns, &arg_anharmonicity_mhz,
        &arg_leakage_weight, &arg_max_amplitude_mhz, &arg_n_iterations,
    ])?;
    Ok(Json(result))
}

/// POST /qml/readout-crosstalk — model frequency-multiplexed readout crosstalk via dispersive coupling.
async fn qml_readout_crosstalk(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_resonators = req.get("n_resonators").and_then(as_u64_loose).unwrap_or(5);
    let center_freq_ghz = req.get("center_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(6.5);
    let freq_spacing_mhz = req.get("freq_spacing_mhz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let qubit_freq_ghz = req.get("qubit_freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0);
    let coupling_mhz = req.get("coupling_mhz").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let q_factor = req.get("q_factor").and_then(|v| v.as_f64()).unwrap_or(1000.0);
    let meas_time_ns = req.get("meas_time_ns").and_then(|v| v.as_f64()).unwrap_or(500.0);
    let arg_n_resonators = format!("--n-resonators={n_resonators}");
    let arg_center_freq_ghz = format!("--center-freq-ghz={center_freq_ghz}");
    let arg_freq_spacing_mhz = format!("--freq-spacing-mhz={freq_spacing_mhz}");
    let arg_qubit_freq_ghz = format!("--qubit-freq-ghz={qubit_freq_ghz}");
    let arg_coupling_mhz = format!("--coupling-mhz={coupling_mhz}");
    let arg_q_factor = format!("--q-factor={q_factor}");
    let arg_meas_time_ns = format!("--meas-time-ns={meas_time_ns}");
    let result = run_tool("qml", &[
        "readout-crosstalk", "--json",
        &arg_n_resonators, &arg_center_freq_ghz, &arg_freq_spacing_mhz,
        &arg_qubit_freq_ghz, &arg_coupling_mhz, &arg_q_factor, &arg_meas_time_ns,
    ])?;
    Ok(Json(result))
}

/// POST /qion/raman-cool — simulate Raman sideband cooling to ground state.
async fn qion_raman_cool(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_rounds = req.get("n_rounds").and_then(as_u64_loose).unwrap_or(20);
    let time_per_round_ms = req.get("time_per_round_ms").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let eta = req.get("eta").and_then(|v| v.as_f64()).unwrap_or(0.1);
    let rabi_freq_khz = req.get("rabi_freq_khz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let linewidth_khz = req.get("linewidth_khz").and_then(|v| v.as_f64()).unwrap_or(0.01);
    let heating_rate = req.get("heating_rate").and_then(|v| v.as_f64()).unwrap_or(0.5);
    let initial_mean_phonon = req.get("initial_mean_phonon").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let n_max = req.get("n_max").and_then(as_u64_loose).unwrap_or(100);
    let arg_n_rounds = format!("--n-rounds={n_rounds}");
    let arg_time_per_round_ms = format!("--time-per-round-ms={time_per_round_ms}");
    let arg_eta = format!("--eta={eta}");
    let arg_rabi_freq_khz = format!("--rabi-freq-khz={rabi_freq_khz}");
    let arg_linewidth_khz = format!("--linewidth-khz={linewidth_khz}");
    let arg_heating_rate = format!("--heating-rate-phonons-per-ms={heating_rate}");
    let arg_initial_mean_phonon = format!("--initial-mean-phonon={initial_mean_phonon}");
    let arg_n_max = format!("--n-max={n_max}");
    let result = run_tool("qion", &[
        "raman-cool", "--json",
        &arg_n_rounds, &arg_time_per_round_ms, &arg_eta,
        &arg_rabi_freq_khz, &arg_linewidth_khz, &arg_heating_rate,
        &arg_initial_mean_phonon, &arg_n_max,
    ])?;
    Ok(Json(result))
}

/// POST /qspin/nuclear-bath — simulate spin-qubit dephasing from nuclear spin bath.
async fn qspin_nuclear_bath(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_nuclei = req.get("n_nuclei").and_then(as_u64_loose).unwrap_or(1000);
    let species = req.get("species").and_then(|v| v.as_str()).unwrap_or("Si29").to_string();
    let concentration_ppm = req.get("concentration_ppm").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let a_max_khz = req.get("a_max_khz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let n_samples = req.get("n_samples").and_then(as_u64_loose).unwrap_or(500);
    let t_max_us = req.get("t_max_us").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let n_time_points = req.get("n_time_points").and_then(as_u64_loose).unwrap_or(50);
    let arg_n_nuclei = format!("--n-nuclei={n_nuclei}");
    let arg_species = format!("--species={species}");
    let arg_concentration_ppm = format!("--concentration-ppm={concentration_ppm}");
    let arg_a_max_khz = format!("--a-max-khz={a_max_khz}");
    let arg_n_samples = format!("--n-samples={n_samples}");
    let arg_t_max_us = format!("--t-max-us={t_max_us}");
    let arg_n_time_points = format!("--n-time-points={n_time_points}");
    let result = run_tool("qspin", &[
        "nuclear-bath", "--json",
        &arg_n_nuclei, &arg_species, &arg_concentration_ppm,
        &arg_a_max_khz, &arg_n_samples, &arg_t_max_us, &arg_n_time_points,
    ])?;
    Ok(Json(result))
}

/// POST /bbq/jpa-model — Josephson parametric amplifier gain/noise model via BBQ.
async fn bbq_jpa_model(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let omega_s_ghz = req.get("omega_s_ghz").and_then(|v| v.as_f64()).unwrap_or(6.0);
    let kappa_s_mhz = req.get("kappa_s_mhz").and_then(|v| v.as_f64()).unwrap_or(10.0);
    let g3_mhz = req.get("g3_mhz").and_then(|v| v.as_f64()).unwrap_or(50.0);
    let pump_power_ratio = req.get("pump_power_ratio").and_then(|v| v.as_f64()).unwrap_or(0.8);
    let n_sweep = req.get("n_sweep").and_then(as_u64_loose).unwrap_or(30);
    let temperature_mk = req.get("temperature_mk").and_then(|v| v.as_f64()).unwrap_or(20.0);
    let arg_omega_s_ghz = format!("--omega-s-ghz={omega_s_ghz}");
    let arg_kappa_s_mhz = format!("--kappa-s-mhz={kappa_s_mhz}");
    let arg_g3_mhz = format!("--g3-mhz={g3_mhz}");
    let arg_pump_power_ratio = format!("--pump-power-ratio={pump_power_ratio}");
    let arg_n_sweep = format!("--n-sweep={n_sweep}");
    let arg_temperature_mk = format!("--temperature-mk={temperature_mk}");
    let result = run_tool("rustybbq", &[
        "jpa-model", "--json",
        &arg_omega_s_ghz, &arg_kappa_s_mhz, &arg_g3_mhz,
        &arg_pump_power_ratio, &arg_n_sweep, &arg_temperature_mk,
    ])?;
    Ok(Json(result))
}

/// POST /orchestrate/xeb-verify — cross-entropy benchmarking verification via orchestrator.
async fn orchestrate_xeb_verify(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_qubits = req.get("n_qubits").and_then(as_u64_loose).unwrap_or(10);
    let n_circuits = req.get("n_circuits").and_then(as_u64_loose).unwrap_or(20);
    let n_shots = req.get("n_shots").and_then(as_u64_loose).unwrap_or(1000);
    let n_gates = req.get("n_gates").and_then(as_u64_loose).unwrap_or(40);
    let fidelity = req.get("fidelity").and_then(|v| v.as_f64()).unwrap_or(0.995);
    let seed = req.get("seed").and_then(as_u64_loose).unwrap_or(42);
    let arg_n_qubits = format!("--n-qubits={n_qubits}");
    let arg_n_circuits = format!("--n-circuits={n_circuits}");
    let arg_n_shots = format!("--n-shots={n_shots}");
    let arg_n_gates = format!("--n-gates={n_gates}");
    let arg_fidelity = format!("--fidelity={fidelity}");
    let arg_seed = format!("--seed={seed}");
    let result = run_tool("qorchestrate", &[
        "xeb-verify", "--json",
        &arg_n_qubits, &arg_n_circuits, &arg_n_shots,
        &arg_n_gates, &arg_fidelity, &arg_seed,
    ])?;
    Ok(Json(result))
}

/// POST /transpile/xtalk-map — build crosstalk-aware qubit mapping via transpiler.
async fn transpile_xtalk_map(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let n_qubits = req.get("n_qubits").and_then(as_u64_loose).unwrap_or(8);
    let n_gates = req.get("n_gates").and_then(as_u64_loose).unwrap_or(20);
    let xtalk_base_mhz = req.get("xtalk_base_mhz").and_then(|v| v.as_f64()).unwrap_or(0.05);
    let decay_length = req.get("decay_length").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let n_trials = req.get("n_trials").and_then(as_u64_loose).unwrap_or(1000);
    let seed = req.get("seed").and_then(as_u64_loose).unwrap_or(42);
    let arg_n_qubits = format!("--n-qubits={n_qubits}");
    let arg_n_gates = format!("--n-gates={n_gates}");
    let arg_xtalk_base_mhz = format!("--xtalk-base-mhz={xtalk_base_mhz}");
    let arg_decay_length = format!("--decay-length={decay_length}");
    let arg_n_trials = format!("--n-trials={n_trials}");
    let arg_seed = format!("--seed={seed}");
    let result = run_tool("transpile", &[
        "--json", "xtalk-map",
        &arg_n_qubits, &arg_n_gates, &arg_xtalk_base_mhz,
        &arg_decay_length, &arg_n_trials, &arg_seed,
    ])?;
    Ok(Json(result))
}

/// POST /bbq/to-qcirc — BbqQuantResult → rustyqcirc HamiltonianSpec.
///
/// Accepts the BbqQuantResult that `rustybbq quantize` returns (canonical
/// field name `bbq_result`; auto-finds an `<upstream>_output` carrying
/// `mode_frequencies_ghz` as the BBQ marker). Optional `n_levels` is a
/// comma-separated Fock truncation list (last value repeats); optional
/// `description` is forwarded verbatim.
async fn bbq_to_qcirc(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let bbq_result = req
        .get("bbq_result")
        .cloned()
        .or_else(|| find_field_by_marker(&req, "mode_frequencies_ghz").cloned())
        .or_else(|| find_field_by_marker(&req, "poles").cloned())
        .ok_or_else(|| {
            anyhow::anyhow!("missing bbq_result (or <upstream>_output carrying BBQ poles)")
        })?;
    let in_f = write_temp_json(&bbq_result)?;
    let mut args: Vec<String> = vec![
        "--json".into(),
        "to-qcirc".into(),
        "--input".into(),
        in_f.path().to_str().unwrap().to_string(),
    ];
    if let Some(s) = req.get("n_levels").and_then(|v| v.as_str()) {
        args.extend(["--n-levels".into(), s.to_string()]);
    } else if let Some(arr) = req.get("n_levels").and_then(|v| v.as_array()) {
        let s = arr
            .iter()
            .filter_map(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if !s.is_empty() {
            args.extend(["--n-levels".into(), s]);
        }
    }
    if let Some(s) = req.get("description").and_then(|v| v.as_str()) {
        args.extend(["--description".into(), s.to_string()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("rustybbq", &args_ref)?;
    Ok(Json(result))
}

// ---------------------------------------------------------------------------
// qcirc — parametric process engine
// ---------------------------------------------------------------------------
//
// Wires `quantum-qcirc/target/release/qcirc` into the gateway. The 7 endpoints
// mirror the 7 `qcirc` subcommands the `parametric-process-design.toml`
// orchestrate template references. Every endpoint accepts a JSON body, writes
// any structured inputs to temp files, calls the CLI with `--json`, and
// returns the parsed response.
//
// Data wiring between orchestrate stages: when a stage runs, the executor
// injects each dependency's output as `<dep_id>_output` in the next stage's
// request body. The helpers below scan for those keys so the template doesn't
// need explicit `remap`. A field whose value object contains "modes" (a
// QuantizedCircuit / HamiltonianSpec) is taken as the circuit; one with
// "feasible_points" is the regime-scan result; one with "pump_tones" is a
// pump config.

fn find_field_by_marker<'a>(req: &'a Value, marker: &str) -> Option<&'a Value> {
    if let Value::Object(map) = req {
        // Prefer the canonical name if present.
        for k in ["hamiltonian", "circuit", "regime_result", "pumps", "pump_config"] {
            if let Some(v) = map.get(k)
                && v.get(marker).is_some()
            {
                return Some(v);
            }
        }
        // Else scan for any `<dep>_output` whose value carries the marker.
        for (k, v) in map {
            if k.ends_with("_output") && v.get(marker).is_some() {
                return Some(v);
            }
        }
    }
    None
}

fn find_circuit(req: &Value) -> Option<&Value> {
    find_field_by_marker(req, "modes")
}

/// Scan request body for an `<upstream>_output` whose value is a JSON array
/// of objects carrying both `freq_ghz` and `pump_operator` — the bare shape
/// `qcirc pump-design` serializes (a `Vec<PumpSpec>`). Returns the array.
fn upstream_pump_specs(req: &Value) -> Option<&Vec<Value>> {
    if let Value::Object(map) = req {
        for (k, v) in map {
            if !k.ends_with("_output") { continue; }
            if let Some(arr) = v.as_array() {
                // Recognize a non-empty PumpSpec[] by the first element's
                // shape; an empty array is accepted too so the downstream
                // can short-circuit gracefully.
                if arr.is_empty() {
                    return Some(arr);
                }
                if arr.first().is_some_and(|s|
                    s.get("freq_ghz").is_some() && s.get("pump_operator").is_some()
                ) {
                    return Some(arr);
                }
            }
        }
    }
    None
}

fn find_regime_result(req: &Value) -> Option<&Value> {
    find_field_by_marker(req, "feasible_points")
        .or_else(|| find_field_by_marker(req, "pareto_front"))
}

/// POST /qcirc/quantize — netlist → QuantizedCircuit.
///
/// Accepts: `{ "netlist": <CircuitNetlist JSON>, "taylor_order": int,
///            "n_levels": "5,5,4" (optional) }`.
async fn qcirc_quantize(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let netlist = req
        .get("netlist")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing field: netlist"))?;
    let netlist_f = write_temp_json(&netlist)?;
    let order = req
        .get("taylor_order")
        .or_else(|| req.get("order"))
        .and_then(as_u64_loose)
        .unwrap_or(4)
        .to_string();
    let n_levels = req.get("n_levels").and_then(|v| v.as_str());
    let mut args: Vec<String> = vec![
        "quantize".into(),
        "--json".into(),
        "--netlist".into(),
        netlist_f.path().to_str().unwrap().to_string(),
        "--order".into(),
        order,
    ];
    if let Some(s) = n_levels {
        args.extend(["--n-levels".into(), s.to_string()]);
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qcirc", &args_ref)?;
    Ok(Json(result))
}

/// POST /qcirc/processes — circuit → identified parametric processes.
async fn qcirc_processes(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = find_circuit(&req)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing circuit (hamiltonian / <upstream>_output)"))?;
    let f = write_temp_json(&circuit)?;
    let tolerance = req
        .get("tolerance_ghz")
        .or_else(|| req.get("tolerance"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.1)
        .to_string();
    let result = run_tool(
        "qcirc",
        &[
            "processes",
            "--json",
            "--hamiltonian",
            f.path().to_str().unwrap(),
            "--tolerance",
            &tolerance,
        ],
    )?;
    Ok(Json(result))
}

/// POST /qcirc/pump-design — circuit → pump frequencies for parametric drives.
async fn qcirc_pump_design(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = find_circuit(&req)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing circuit (hamiltonian / <upstream>_output)"))?;
    let f = write_temp_json(&circuit)?;
    let process = req
        .get("process")
        .or_else(|| req.get("target_process"))
        .and_then(|v| v.as_str())
        .unwrap_or("3wm_diff");
    let result = run_tool(
        "qcirc",
        &[
            "pump-design",
            "--json",
            "--hamiltonian",
            f.path().to_str().unwrap(),
            "--process",
            process,
        ],
    )?;
    Ok(Json(result))
}

/// POST /qcirc/floquet — circuit + pump config → Floquet quasi-energies + couplings.
async fn qcirc_floquet(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = find_circuit(&req)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing circuit (hamiltonian / <upstream>_output)"))?;
    // Look for an explicit PumpConfiguration first (canonical `pumps` or
    // `pump_config`, or an upstream output already carrying `tones`).
    // Otherwise upgrade `pump_specs` (the shape `/qcirc/pump-design` returns)
    // into a PumpConfiguration with default amplitude / phase / target_mode —
    // each PumpSpec lacks those fields so the bridge is necessarily lossy.
    let pumps = if let Some(p) = req.get("pumps").cloned() {
        p
    } else if let Some(p) = req.get("pump_config").cloned() {
        p
    } else if let Some(p) = find_field_by_marker(&req, "tones").cloned() {
        p
    } else if let Some(specs) = upstream_pump_specs(&req) {
        // `qcirc pump-design` returns a bare PumpSpec[] (NOT a {pump_specs:[…]}
        // object), so the orchestrator stores it as `qcirc_pump_design_output:
        // [PumpSpec, …]`. Lift each spec into a PumpTone, supplying
        // amplitude / phase / target_mode that PumpSpec doesn't carry.
        let tones: Vec<Value> = specs
            .iter()
            .map(|s| {
                serde_json::json!({
                    "freq_ghz": s.get("freq_ghz").cloned().unwrap_or(Value::from(5.0)),
                    "amplitude": 0.01,
                    "phase_rad": 0.0,
                    "pump_type": s.get("pump_operator").cloned().unwrap_or(Value::from("FluxModulation")),
                    "target_mode": 0_u64,
                })
            })
            .collect();
        serde_json::json!({ "tones": tones })
    } else {
        return Err(anyhow::anyhow!("missing pumps (PumpConfiguration JSON)").into());
    };

    // Upstream pump-design produced no specs (sparse Hamiltonian) → return
    // an empty Floquet result so downstream `[stage.condition]` blocks can
    // skip cleanly. Without this short-circuit the qcirc binary would error
    // on PumpConfiguration { tones: [] } and the whole pipeline aborts.
    if pumps.get("tones").and_then(|t| t.as_array()).is_some_and(|a| a.is_empty()) {
        return Ok(Json(serde_json::json!({
            "quasienergies_ghz": [],
            "effective_couplings": {},
            "collision_analysis": { "collision_free": false, "min_gap_ghz": 0.0 },
            "note": "upstream pump-design produced no specs — Floquet skipped"
        })));
    }

    let circuit_f = write_temp_json(&circuit)?;
    let pumps_f = write_temp_json(&pumps)?;
    let mut args: Vec<String> = vec![
        "floquet".into(),
        "--json".into(),
        "--hamiltonian".into(),
        circuit_f.path().to_str().unwrap().to_string(),
        "--pumps".into(),
        pumps_f.path().to_str().unwrap().to_string(),
    ];
    if let Some(s) = req.get("harmonics").and_then(|v| v.as_str()) {
        args.extend(["--harmonics".into(), s.to_string()]);
    } else if let Some(arr) = req.get("n_harmonics").and_then(|v| v.as_array()) {
        let s = arr
            .iter()
            .filter_map(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",");
        if !s.is_empty() {
            args.extend(["--harmonics".into(), s]);
        }
    }
    let args_ref: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
    let result = run_tool("qcirc", &args_ref)?;
    Ok(Json(result))
}

/// POST /qcirc/regime-scan — circuit + sweep spec → Pareto-optimal regime.
async fn qcirc_regime_scan(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let circuit = find_circuit(&req)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing circuit (hamiltonian / <upstream>_output)"))?;
    let f = write_temp_json(&circuit)?;
    let process = req
        .get("process")
        .and_then(|v| v.as_str())
        .unwrap_or("3wm_diff");
    let sweep = req
        .get("sweep")
        .and_then(|v| v.as_str())
        .unwrap_or("flux_dc:0.35:0.45:10,pump_amp:0.001:0.05:15");
    let result = run_tool(
        "qcirc",
        &[
            "regime-scan",
            "--json",
            "--hamiltonian",
            f.path().to_str().unwrap(),
            "--process",
            process,
            "--sweep",
            sweep,
        ],
    )?;
    Ok(Json(result))
}

/// POST /qcirc/constraints — regime scan result → circuit parameter constraints.
async fn qcirc_constraints(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let regime = find_regime_result(&req)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("missing regime_result (or <upstream>_output)"))?;
    let f = write_temp_json(&regime)?;
    let result = run_tool(
        "qcirc",
        &[
            "constraints",
            "--json",
            "--regime-result",
            f.path().to_str().unwrap(),
        ],
    )?;
    Ok(Json(result))
}

/// POST /qcirc/summary — aggregate the prior stages' outputs into one report.
///
/// Pure JSON aggregation: gathers any `<upstream>_output` keys (constraints,
/// floquet, processes, …) and emits a compact `summary` plus the raw upstream
/// dicts. No subprocess call.
async fn qcirc_summary(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let processes = req
        .as_object()
        .and_then(|m| m.iter().find(|(k, v)| k.contains("processes") && v.is_object().then_some(true).unwrap_or(false)))
        .map(|(_, v)| v.clone());
    // Simpler: just enumerate every *_output and bundle.
    let mut bundle = serde_json::Map::new();
    let mut summary = serde_json::Map::new();
    if let Value::Object(map) = &req {
        for (k, v) in map {
            if k.ends_with("_output") {
                bundle.insert(k.clone(), v.clone());
                // Cherry-pick a few headline fields into the summary.
                if let Some(n) = v.get("processes").and_then(|p| p.as_array()).map(|a| a.len()) {
                    summary.insert("n_processes_identified".into(), Value::from(n));
                }
                if let Some(p) = v.get("pump_specs").and_then(|p| p.as_array()).and_then(|a| a.first()) {
                    if let Some(f) = p.get("freq_ghz").and_then(|v| v.as_f64()) {
                        summary.insert("dominant_pump_freq_ghz".into(), Value::from(f));
                    }
                }
                if let Some(b) = v.get("collision_analysis").and_then(|c| c.get("collision_free")).and_then(|v| v.as_bool()) {
                    summary.insert("floquet_collision_free".into(), Value::from(b));
                }
                if let Some(a) = v.get("pareto_front").and_then(|p| p.as_array()).map(|a| a.len()) {
                    summary.insert("n_pareto_regime_points".into(), Value::from(a));
                }
                if let Some(c) = v.get("constraints").and_then(|c| c.as_array()).map(|a| a.len()) {
                    summary.insert("n_circuit_constraints".into(), Value::from(c));
                }
            }
        }
    }
    let _ = processes; // currently unused; reserved for future header expansion
    Ok(Json(serde_json::json!({
        "summary": Value::Object(summary),
        "upstream": Value::Object(bundle),
    })))
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

fn build_router() -> Router {
    Router::new()
        .route("/health", get(health))
        // qtwin
        .route("/qtwin/compare", post(qtwin_compare))
        .route("/qtwin/:chip/compare", get(qtwin_compare_chip))
        .route("/qtwin/ingest", post(qtwin_ingest))
        .route("/qtwin/acquire", post(qtwin_acquire))
        .route("/qtwin/characterize", post(qtwin_characterize))
        .route("/qtwin/recalibrate", post(qtwin_recalibrate))
        .route("/qtwin/qec-update", post(qtwin_qec_update))
        .route("/qtwin/mock", post(qtwin_mock))
        // freq
        .route("/freq/optimize", post(freq_optimize))
        .route("/freq/check", post(freq_check))
        .route("/freq/yield", post(freq_yield))
        // xtalk
        .route("/xtalk/coupling", post(xtalk_coupling))
        .route("/xtalk/zz", post(xtalk_zz))
        .route("/xtalk/crosstalk", post(xtalk_crosstalk))
        .route("/xtalk/simulate", post(xtalk_simulate))
        .route("/xtalk/tunable-coupler", post(xtalk_tunable_coupler))
        // readout
        .route("/readout/health", get(readout_health))
        .route("/readout/design", post(readout_design))
        .route("/readout/multiplex", post(readout_multiplex))
        .route("/readout/optimize", post(readout_optimize))
        .route("/readout/fidelity", post(readout_fidelity))
        .route("/readout/mist", post(readout_mist))
        .route("/readout/erasure", post(readout_erasure))
        .route("/readout/reset", post(readout_reset))
        .route("/readout/feedforward", post(readout_feedforward))
        .route("/readout/leakage", post(readout_leakage))
        .route("/readout/paramp", post(readout_paramp))
        // bench
        .route("/bench/health", get(bench_health))
        .route("/bench/predict", post(bench_predict))
        .route("/bench/suggest", post(bench_suggest))
        .route("/bench/qv", post(bench_qv))
        .route("/bench/rb", post(bench_rb))
        .route("/bench/compare", post(bench_compare))
        // qstar
        .route("/qstar/threshold", post(qstar_threshold))
        // surgery
        .route("/surgery/health", get(surgery_health))
        .route("/surgery/resources", post(surgery_resources))
        .route("/surgery/factory", post(surgery_factory))
        .route("/surgery/compile", post(surgery_compile))
        .route("/surgery/visualize", post(surgery_visualize))
        // qexplore
        .route("/qexplore/sweep", post(qexplore_sweep))
        .route("/qexplore/fridge", post(qexplore_fridge))
        // xtalk simple ZZ analysis
        .route("/xtalk/zz-simple", post(xtalk_zz_simple))
        // pulse simulation
        .route("/pulse/simulate", post(pulse_simulate))
        // stim circuits
        .route("/stim/gen", post(stim_gen))
        .route("/stim/circuit", post(stim_circuit))
        .route("/stim/ldpc", post(stim_ldpc))
        .route("/stim/xzzx", post(stim_xzzx))
        // design pipeline
        .route("/pipeline/design", post(pipeline_design))
        // qpudidp direct proxy (new device types + rmflow)
        .route("/qpudidp/inverse-design-rmflow", post(qpudidp_inverse_design_rmflow))
        .route("/qpudidp/paired-design-predict", post(qpudidp_paired_design_predict))
        .route("/qpudidp/rectangular-cavity-3d-predict", post(qpudidp_rectangular_cavity_3d_predict))
        .route("/qpudidp/uncertainty-quantile", post(qpudidp_uncertainty_quantile))
        // qem analytical proxy
        .route("/qem/solve_lom", post(qem_solve_lom))
        .route("/qem/solve_lom_tunable", post(qem_solve_lom_tunable))
        .route("/qem/solve_lom_cavity", post(qem_solve_lom_cavity))
        .route("/qem/solve_cavity_transmon", post(qem_solve_cavity_transmon))
        .route("/qem/antenna_sweep", post(qem_antenna_sweep))
        .route("/qem/sweep", post(qem_sweep))
        .route("/qem/sparams", post(qem_sparams))
        // rustybbq
        .route("/bbq/health", get(bbq_health))
        .route("/bbq/quantize", post(bbq_quantize))
        .route("/bbq/bus", post(bbq_bus))
        .route("/bbq/hamiltonian", post(bbq_hamiltonian))
        .route("/bbq/zz-coupling", post(bbq_zz_coupling))
        .route("/bbq/coupler-zz", post(bbq_coupler_zz))
        // qcirc — parametric process engine (netlist → quantize → processes →
        // pump-design → floquet → regime-scan → constraints)
        .route("/qcirc/quantize", post(qcirc_quantize))
        .route("/qcirc/processes", post(qcirc_processes))
        .route("/qcirc/pump-design", post(qcirc_pump_design))
        .route("/qcirc/floquet", post(qcirc_floquet))
        .route("/qcirc/regime-scan", post(qcirc_regime_scan))
        .route("/qcirc/constraints", post(qcirc_constraints))
        .route("/qcirc/summary", post(qcirc_summary))
        // bbq → qcirc bridge (Foster-quantization output → HamiltonianSpec)
        .route("/bbq/to-qcirc", post(bbq_to_qcirc))
        // rustyfloquet
        .route("/floquet/health", get(floquet_health))
        .route("/floquet/spectrum", post(floquet_spectrum))
        .route("/floquet/propagator", post(floquet_propagator))
        .route("/floquet/lindblad", post(floquet_lindblad_endpoint))
        .route("/floquet/bbq-floquet", post(floquet_bbq_floquet_endpoint))
        .route("/floquet/grape", post(floquet_grape_endpoint))
        .route("/floquet/flime-solve", post(floquet_flime_solve_endpoint))
        // rustyqml
        .route("/qml/health", get(qml_health))
        .route("/qml/classify", post(qml_classify))
        .route("/qml/kernel", post(qml_kernel))
        .route("/qml/resources", post(qml_resources))
        .route("/qml/barren-plateau", post(qml_barren_plateau))
        // rustycryo
        .route("/cryo/health", get(cryo_health))
        .route("/cryo/analyze", post(cryo_analyze))
        .route("/cryo/power", post(cryo_power))
        .route("/cryo/compare", post(cryo_compare))
        .route("/cryo/scale", post(cryo_scale))
        // rustyqnet
        .route("/qnet/health", get(qnet_health))
        .route("/qnet/analyze", post(qnet_analyze))
        .route("/qnet/entangle", post(qnet_entangle))
        .route("/qnet/scale", post(qnet_scale))
        .route("/qnet/compare-links", post(qnet_compare_links))
        // rustyqchem
        .route("/qchem/health", get(qchem_health))
        .route("/qchem/molecule", post(qchem_molecule))
        .route("/qchem/vqe", post(qchem_vqe))
        .route("/qchem/resources", post(qchem_resources))
        // rustycryo-wiring
        .route("/wiring/health", get(wiring_health))
        .route("/wiring/design", post(wiring_design))
        .route("/wiring/noise", post(wiring_noise))
        .route("/wiring/scale", post(wiring_scale))
        .route("/wiring/optimize", post(wiring_optimize))
        // rustyextract
        .route("/extract/health", get(extract_health))
        .route("/extract/cpw", post(extract_cpw))
        .route("/extract/tls", post(extract_tls))
        // rustyqatom
        .route("/qatom/health", get(qatom_health))
        .route("/qatom/design", post(qatom_design))
        .route("/qatom/gate", post(qatom_gate))
        .route("/qatom/blockade", post(qatom_blockade))
        .route("/qatom/loading", post(qatom_loading))
        .route("/qatom/multi-gate", post(qatom_multi_gate))
        .route("/qatom/zone-layout", post(qatom_zone_layout))
        .route("/qatom/coherence", post(qatom_coherence))
        .route("/qatom/readout", post(qatom_readout))
        .route("/qatom/crosstalk", post(qatom_crosstalk))
        .route("/qatom/frequency", post(qatom_frequency))
        .route("/qatom/pulse", post(qatom_pulse))
        // rustypulse-qec
        .route("/pqec/health", get(pqec_health))
        .route("/pqec/assess", post(pqec_assess))
        .route("/pqec/threshold", post(pqec_threshold))
        .route("/pqec/overhead", post(pqec_overhead))
        .route("/pqec/sweep", post(pqec_sweep))
        // rustyqspin
        .route("/qspin/health", get(qspin_health))
        .route("/qspin/design", post(qspin_design))
        .route("/qspin/fidelity", post(qspin_fidelity))
        .route("/qspin/stability", post(qspin_stability))
        .route("/qspin/fab", post(qspin_fab))
        .route("/qspin/yield", post(qspin_yield))
        .route("/qspin/valley-split", post(qspin_valley_split))
        .route("/qspin/coherence", post(qspin_coherence))
        .route("/qspin/readout", post(qspin_readout))
        .route("/qspin/crosstalk", post(qspin_crosstalk))
        .route("/qspin/frequency", post(qspin_frequency))
        .route("/qspin/pulse", post(qspin_pulse))
        // rustyqion
        .route("/qion/health", get(qion_health))
        .route("/qion/design", post(qion_design))
        .route("/qion/ms-gate", post(qion_ms_gate))
        .route("/qion/modes", post(qion_modes))
        .route("/qion/cooling", post(qion_cooling))
        .route("/qion/schedule", post(qion_schedule))
        .route("/qion/coherence", post(qion_coherence))
        .route("/qion/readout", post(qion_readout))
        .route("/qion/crosstalk", post(qion_crosstalk))
        .route("/qion/frequency", post(qion_frequency))
        .route("/qion/pulse", post(qion_pulse))
        // rustybosonic
        .route("/bosonic/health", get(bosonic_health))
        .route("/bosonic/simulate", post(bosonic_simulate))
        .route("/bosonic/compare", post(bosonic_compare))
        .route("/bosonic/optimize", post(bosonic_optimize))
        .route("/bosonic/break-even", post(bosonic_break_even))
        .route("/bosonic/concat", post(bosonic_concat))
        // rustycodesign
        .route("/codesign/health", get(codesign_health))
        .route("/codesign/optimize", post(codesign_optimize))
        .route("/codesign/roadmap", post(codesign_roadmap))
        .route("/codesign/compare-platforms", post(codesign_compare_platforms))
        .route("/codesign/what-if", post(codesign_what_if))
        .route("/codesign/sensitivity", post(codesign_sensitivity))
        .route("/qec/compile", post(qec_compile))
        // rustyqopt (QAOA)
        .route("/qaoa/health", get(qaoa_health))
        .route("/qaoa/maxcut", post(qaoa_maxcut))
        .route("/qaoa/portfolio", post(qaoa_portfolio))
        .route("/qaoa/tsp", post(qaoa_tsp))
        .route("/qaoa/resources", post(qaoa_resources))
        // rustycal
        .route("/cal/health", get(cal_health))
        .route("/cal/spectroscopy", post(cal_spectroscopy))
        .route("/cal/rabi", post(cal_rabi))
        .route("/cal/t1", post(cal_t1))
        .route("/cal/rb", post(cal_rb))
        .route("/cal/cycle-rb", post(cal_cycle_rb))
        .route("/cal/adaptive", post(cal_adaptive))
        // rustyqfw
        .route("/qfw/health", get(qfw_health))
        .route("/qfw/compile", post(qfw_compile))
        .route("/qfw/schedule", post(qfw_schedule))
        .route("/qfw/simulate", post(qfw_simulate))
        .route("/qfw/export", post(qfw_export))
        // rustytranspile
        .route("/transpile/health", get(transpile_health))
        .route("/transpile/compile", post(transpile_compile))
        .route("/transpile/analyze", post(transpile_analyze))
        .route("/transpile/noise-aware", post(transpile_noise_aware))
        .route("/transpile/compare", post(transpile_compare))
        // rustypkg
        .route("/pkg/health", get(pkg_health))
        .route("/pkg/design", post(pkg_design_endpoint))
        .route("/pkg/box-modes", post(pkg_box_modes_endpoint))
        .route("/pkg/wirebonds", post(pkg_wirebonds_endpoint))
        .route("/pkg/export", post(pkg_export_endpoint))
        // rustyswap
        .route("/swap/health", get(swap_health))
        .route("/swap/figure1d", post(swap_figure1d))
        .route("/swap/figure3a", post(swap_figure3a))
        .route("/swap/figure3c", post(swap_figure3c))
        .route("/swap/figure4c", post(swap_figure4c))
        .route("/swap/fock-convergence", post(swap_fock_convergence))
        .route("/swap/sw-validity", post(swap_sw_validity))
        .route("/swap/nmodule-chain", post(swap_nmodule_chain))
        .route("/swap/tls-loss", post(swap_tls_loss))
        .route("/swap/chi-sensitivity", post(swap_chi_sensitivity))
        .route("/swap/spam-model", post(swap_spam_model))
        .route("/swap/crosstalk-sweep", post(swap_crosstalk_sweep))
        .route("/swap/param-spread", post(swap_param_spread))
        .route("/swap/nmodule-scaling", post(swap_nmodule_scaling))
        // oqfp
        .route("/oqfp/health", get(oqfp_health))
        .route("/oqfp/validate", post(oqfp_validate_endpoint))
        .route("/oqfp/summary", post(oqfp_summary_endpoint))
        .route("/oqfp/diff", post(oqfp_diff_endpoint))
        .route("/oqfp/create", post(oqfp_create_endpoint))
        // rustyscq
        .route("/scq/health", get(scq_health))
        .route("/scq/spectrum", post(scq_spectrum))
        .route("/scq/simulate", post(scq_simulate))
        .route("/scq/dispersion", post(scq_dispersion))
        .route("/scq/flux-sweep", post(scq_flux_sweep))
        .route("/scq/coherence", post(scq_coherence))
        // orchestrate
        .route("/orchestrate/health", get(orchestrate_health))
        .route("/orchestrate/stages", get(orchestrate_stages))
        .route("/orchestrate/validate", post(orchestrate_validate))
        .route("/orchestrate/run", post(orchestrate_run))

        .route("/symclaw/health", get(symclaw_health))
        .route("/symclaw/simplify", post(symclaw_simplify))
        .route("/symclaw/differentiate", post(symclaw_differentiate))
        .route("/symclaw/integrate", post(symclaw_integrate))
        .route("/symclaw/solve", post(symclaw_solve))
        .route("/symclaw/taylor", post(symclaw_taylor))
        .route("/symclaw/limit", post(symclaw_limit))
        .route("/symclaw/codegen", post(symclaw_codegen))
        .route("/symclaw/linalg", post(symclaw_linalg))
        .route("/symclaw/polynomial", post(symclaw_polynomial))
        .route("/symclaw/analyze", post(symclaw_analyze))
        // claw-mesh (Phase 7X)
        .route("/mesh/health", get(mesh_health))
        .route("/mesh/transmon-cross", post(mesh_transmon_cross))
        .route("/mesh/rectangular-cavity-3d", post(mesh_rectangular_cavity_3d))
        .route("/mesh/tunable-transmon", post(mesh_tunable_transmon))
        .route("/mesh/xmon", post(mesh_xmon))
        .route("/mesh/fluxonium", post(mesh_fluxonium))
        .route("/mesh/cpw-resonator", post(mesh_cpw_resonator))
        .route("/mesh/chip", post(mesh_chip))
        .route("/mesh/quality", post(mesh_quality_endpoint))
        // clawprint device modeling (blueprint displacement Phase 4)
        .route("/clawprint/health", get(clawprint_health))
        .route("/clawprint/dressed", post(clawprint_dressed))
        // claw-gds (Phase 7X)
        .route("/gds/health", get(gds_health))
        .route("/gds/transmon-cross", post(gds_transmon_cross))
        .route("/gds/rectangular-cavity-3d", post(gds_rectangular_cavity_3d))
        .route("/gds/chip-layout", post(gds_chip_layout))
        .route("/gds/export", post(gds_export))
        .route("/gds/export-chip", post(gds_export_chip))
        .route("/drc", post(gds_drc))
        .route("/drc/decks", get(gds_drc_decks))
        .route("/foundry/profiles", get(foundry_profiles))
        .route("/tapeout/layermaps", get(tapeout_layermaps))
        .route("/junction/recipes", get(junction_recipes))
        .route("/junction/recipe", post(junction_recipe_eval))
        .route("/junction/yield", post(junction_yield))
        .route("/junction/budget", post(junction_budget))
        // clawview proxy (Phase 7Y)
        .route("/clawview/health", get(clawview_health))
        .route("/clawview/participation", get(clawview_participation))
        .route("/clawview/streamlines", post(clawview_streamlines))
        .route("/clawview/isosurface", get(clawview_isosurface))
        .route("/clawview/coupling", get(clawview_coupling))
        .route("/clawview/surrogate/predict", post(clawview_surrogate_predict))
        .route("/clawview/cross-section", post(clawview_cross_section))
        .route("/clawview/layout/from-params", post(clawview_layout_from_params))
        .route("/clawview/formats", get(clawview_formats))
        // Phase 8K–8R
        .route("/floquet/grape-su2", post(floquet_grape_su2))
        .route("/qml/readout-crosstalk", post(qml_readout_crosstalk))
        .route("/cal/leakage-rb", post(cal_leakage_rb))
        .route("/qion/raman-cool", post(qion_raman_cool))
        .route("/qspin/nuclear-bath", post(qspin_nuclear_bath))
        .route("/bbq/jpa-model", post(bbq_jpa_model))
        .route("/orchestrate/xeb-verify", post(orchestrate_xeb_verify))
        .route("/transpile/xtalk-map", post(transpile_xtalk_map))

        // ── QCVV (Phase 8B) ──────────────────────────────────────────────────
        .route("/qcvv/health",            get(qcvv_health))
        .route("/qcvv/quantum-volume",    post(qcvv_quantum_volume))
        .route("/qcvv/process-fidelity",  post(qcvv_process_fidelity))
        .route("/qcvv/zne",               post(qcvv_zne))
        .route("/qcvv/clops",             post(qcvv_clops))
        .route("/qcvv/rb-analysis",       post(qcvv_rb_analysis))

        .layer(CorsLayer::permissive())

        .layer(TraceLayer::new_for_http())
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    qservices_common::tracing::init("quantum_api");

    let port: u16 = std::env::var("QUANTUM_API_PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(8765);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await
        .expect("failed to bind");

    tracing::info!("quantum-api listening on {addr}");
    tracing::info!("endpoints: /health, /qtwin/*, /freq/*, /xtalk/*, /readout/*, /bench/*, /qstar/*, /surgery/*, /qexplore/*, /qem/*");

    axum::serve(listener, build_router()).await
        .expect("server error");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::util::ServiceExt as _;

    // ── shared-IR: HamiltonianParams convention bridging ─────────────────────

    /// The Hamiltonian-target resolver must accept all three field-name
    /// conventions (quantum-gaps/SHARED-IR-AUDIT.md) so a cross-subsystem payload
    /// isn't silently defaulted to 5.0 GHz.
    #[test]
    fn resolve_hamiltonian_target_accepts_all_conventions() {
        // quantum-services: qubit_freq_ghz
        assert_eq!(
            resolve_hamiltonian_target(&json!({"qubit_freq_ghz": 5.1, "anharmonicity_mhz": -210.0})),
            (5.1, -210.0)
        );
        // qem-core: qubit_frequency_ghz (the convention the old resolver missed)
        assert_eq!(
            resolve_hamiltonian_target(&json!({"qubit_frequency_ghz": 5.2, "anharmonicity_mhz": -205.0})),
            (5.2, -205.0)
        );
        // qpu-didp-core newtype: qubit_frequency / anharmonicity
        assert_eq!(
            resolve_hamiltonian_target(&json!({"qubit_frequency": 5.3, "anharmonicity": -200.0})),
            (5.3, -200.0)
        );
        // QPUDIDP stage-output shape, with qem-convention fields inside
        let staged = json!({
            "inverse_design_output": {
                "best_candidate": {
                    "predicted_hamiltonian": {"qubit_frequency_ghz": 5.4, "anharmonicity_mhz": -195.0}
                }
            }
        });
        assert_eq!(resolve_hamiltonian_target(&staged), (5.4, -195.0));
        // genuinely-missing target still falls back to the documented default
        assert_eq!(resolve_hamiltonian_target(&json!({})).0, 5.0);
    }

    // ── BBQ health ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bbq_health_returns_ok() {
        let app = build_router();
        let req = Request::builder()
            .method("GET")
            .uri("/bbq/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["tool"], "rustybbq");
    }

    // ── Floquet health ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn floquet_health_returns_ok() {
        let app = build_router();
        let req = Request::builder()
            .method("GET")
            .uri("/floquet/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["tool"], "rustyfloquet");
    }

    // ── BBQ quantize serde roundtrip ────────────────────────────────────────

    #[test]
    fn bbq_quantize_request_serde_roundtrip() {
        // Verify that a BbqConfig-like payload round-trips through serde_json
        let payload = json!({
            "s_params": {
                "port_count": 2,
                "frequencies_ghz": [5.0, 6.0, 7.0],
                "s_re": [[[0.1, 0.0], [0.0, 0.0]], [[0.1, 0.0], [0.0, 0.0]], [[0.1, 0.0], [0.0, 0.0]]],
                "s_im": [[[0.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [0.0, 0.0]]]
            },
            "junction_port_indices": [0],
            "ec": [0.3],
            "n_poles": 5,
            "dw_ghz": 1e-4
        });
        let serialized = serde_json::to_string(&payload).unwrap();
        let deserialized: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(deserialized["ec"][0].as_f64().unwrap(), 0.3);
        assert_eq!(deserialized["n_poles"].as_u64().unwrap(), 5);
    }

    // ── BBQ quantize missing s_params causes 500 ────────────────────────────

    #[tokio::test]
    async fn bbq_quantize_missing_s_params_returns_error() {
        let app = build_router();
        // Send a well-formed JSON body that rustybbq won't accept (no binary available in test)
        // This exercises the error path: run_tool will fail because rustybbq isn't on PATH.
        let body = json!({"junction_port_indices": [0]});
        let req = Request::builder()
            .method("POST")
            .uri("/bbq/quantize")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        // Either 500 (tool not on PATH) or 422 (bad JSON) — both are error responses.
        assert!(
            resp.status() == StatusCode::INTERNAL_SERVER_ERROR
                || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "expected error status, got {}",
            resp.status()
        );
    }

    // ── Floquet spectrum serde roundtrip ────────────────────────────────────

    #[test]
    fn floquet_spectrum_request_serde_roundtrip() {
        let payload = json!({
            "hamiltonian": {
                "dim": 2,
                "omega_drive": 5.0,
                "h0_re": [[0.0, 0.0], [0.0, 0.0]],
                "h0_im": [[0.0, 0.0], [0.0, 0.0]],
                "h1_re": [[0.0, 0.5], [0.5, 0.0]],
                "h1_im": [[0.0, 0.0], [0.0, 0.0]]
            },
            "n_harmonics": 3,
            "n_time_points": 128
        });
        let json_str = serde_json::to_string(&payload).unwrap();
        let back: Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(back["n_harmonics"].as_u64().unwrap(), 3);
        assert_eq!(back["n_time_points"].as_u64().unwrap(), 128);
        assert_eq!(back["hamiltonian"]["dim"].as_u64().unwrap(), 2);
    }

    // ── Floquet propagator serde roundtrip ─────────────────────────────────

    #[test]
    fn floquet_propagator_request_serde_roundtrip() {
        let payload = json!({
            "hamiltonian": {
                "dim": 2,
                "omega_drive": 5.0,
                "h0_re": [[2.5, 0.0], [0.0, -2.5]],
                "h0_im": [[0.0, 0.0], [0.0, 0.0]],
                "h1_re": [[0.0, 0.1], [0.1, 0.0]],
                "h1_im": [[0.0, 0.0], [0.0, 0.0]]
            },
            "dt_ns": 0.01,
            "method": "rk4"
        });
        let json_str = serde_json::to_string(&payload).unwrap();
        let back: Value = serde_json::from_str(&json_str).unwrap();
        assert!((back["dt_ns"].as_f64().unwrap() - 0.01).abs() < 1e-12);
        assert_eq!(back["method"].as_str().unwrap(), "rk4");
    }

    // ── BBQ bus missing required field ─────────────────────────────────────

    #[tokio::test]
    async fn bbq_bus_missing_z0_returns_500() {
        let app = build_router();
        // Missing z0 — run_tool or anyhow::anyhow error will produce 500
        let body = json!({"length": 0.1, "freq_start": 4.0, "freq_stop": 8.0});
        let req = Request::builder()
            .method("POST")
            .uri("/bbq/bus")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(body.get("error").is_some(), "response should contain error field");
    }

    // ── General health includes rustybbq, rustyfloquet, rustypkg ─────────────

    #[tokio::test]
    async fn global_health_lists_new_tools() {
        let app = build_router();
        let req = Request::builder()
            .method("GET")
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let tools = body["tools"].as_array().unwrap();
        let tool_names: Vec<&str> = tools.iter().filter_map(|v| v["tool"].as_str()).collect();
        assert!(tool_names.contains(&"rustybbq"), "health should list rustybbq");
        assert!(tool_names.contains(&"rustyfloquet"), "health should list rustyfloquet");
        assert!(tool_names.contains(&"rustypkg"), "health should list rustypkg");
    }

    // ── Pkg health ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn pkg_health_returns_ok() {
        let app = build_router();
        let req = Request::builder()
            .method("GET")
            .uri("/pkg/health")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(body["status"], "ok");
        assert_eq!(body["tool"], "rustypkg");
    }

    // ── Pkg design missing params returns 500 ──────────────────────────────

    #[tokio::test]
    async fn pkg_design_missing_params_returns_error() {
        let app = build_router();
        // Missing housing_length_mm — should 500 with error field
        let body = json!({"n_sma_ports": 4});
        let req = Request::builder()
            .method("POST")
            .uri("/pkg/design")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.get("error").is_some());
    }

    // ── Pkg box-modes missing params returns 500 ───────────────────────────

    #[tokio::test]
    async fn pkg_box_modes_missing_params_returns_error() {
        let app = build_router();
        let body = json!({"band_low_ghz": 4.0});
        let req = Request::builder()
            .method("POST")
            .uri("/pkg/box-modes")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.get("error").is_some());
    }

    // ── Pkg design assembly builder serde round-trip ────────────────────────

    #[test]
    fn pkg_simple_params_assembly_json_is_valid() {
        // Verify the minimal assembly we would build is valid JSON with required fields
        let hl = 50.0_f64;
        let hw = 40.0_f64;
        let hh = 15.0_f64;
        let wall_mm = 5.0_f64;
        let lid_mm = 3.0_f64;
        let assembly = json!({
            "housing": {
                "internal_length_m": (hl - 2.0 * wall_mm) / 1000.0,
                "internal_width_m": (hw - 2.0 * wall_mm) / 1000.0,
                "internal_depth_m": (hh - lid_mm) / 1000.0,
                "wall_thickness_m": wall_mm / 1000.0,
                "lid_thickness_m": lid_mm / 1000.0,
                "chip_recess_depth_m": 0.0005,
                "chip_recess_length_m": 0.035,
                "chip_recess_width_m": 0.028,
                "material": "Aluminum6061"
            },
            "sma_connectors": [],
            "wirebonds": null,
            "indium_seal": null,
            "screw_pattern": null
        });
        let serialized = serde_json::to_string(&assembly).unwrap();
        let back: Value = serde_json::from_str(&serialized).unwrap();
        let il = back["housing"]["internal_length_m"].as_f64().unwrap();
        assert!((il - 0.040).abs() < 1e-9);
    }

    // ── Pkg wirebonds missing chip_pads returns 500 ────────────────────────

    #[tokio::test]
    async fn pkg_wirebonds_missing_chip_pads_returns_error() {
        let app = build_router();
        // Empty body has no chip_pads — handler bails before reaching run_tool.
        let body = json!({});
        let req = Request::builder()
            .method("POST")
            .uri("/pkg/wirebonds")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.get("error").is_some(), "response must contain error field");
    }

    // ── Pkg wirebonds request serde roundtrip ──────────────────────────────

    #[test]
    fn pkg_wirebonds_request_serde_roundtrip() {
        let payload = json!({
            "chip_pads": [
                {"name": "Q0_in", "position_m": {"x": 0.001, "y": 0.002}, "width_m": 0.0001}
            ],
            "pcb_pads": [
                {"name": "P0", "position_m": {"x": 0.005, "y": 0.002}, "width_m": 0.0002}
            ],
            "wire_diameter_m": 2.5e-5,
            "wire_loop_height_m": 1.5e-4,
            "max_wire_length_m": 3.0e-3
        });
        let serialized = serde_json::to_string(&payload).unwrap();
        let back: Value = serde_json::from_str(&serialized).unwrap();
        assert_eq!(back["chip_pads"].as_array().unwrap().len(), 1);
        assert_eq!(back["chip_pads"][0]["name"].as_str().unwrap(), "Q0_in");
        assert!((back["wire_diameter_m"].as_f64().unwrap() - 2.5e-5).abs() < 1e-30);
        assert!((back["max_wire_length_m"].as_f64().unwrap() - 3.0e-3).abs() < 1e-15);
    }

    // ── Pkg export missing assembly returns 500 ────────────────────────────

    #[tokio::test]
    async fn pkg_export_missing_assembly_returns_error() {
        let app = build_router();
        // Body has no assembly key — handler bails before reaching run_tool.
        let body = json!({"formats": ["stl"]});
        let req = Request::builder()
            .method("POST")
            .uri("/pkg/export")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let parsed: Value = serde_json::from_slice(&bytes).unwrap();
        assert!(parsed.get("error").is_some(), "response must contain error field");
    }

    // ── Phonon dephasing rate ────────────────────────────────────────────

    #[test]
    fn phonon_dephasing_rate_typical_transmon() {
        // Typical transmon: alpha=1e-4, cutoff=10 GHz, T=20 mK, f_q=5 GHz
        let rate = phonon_dephasing_rate_mhz_inline(1e-4, 10.0, 20.0, 5.0);
        // Rate must be positive and finite
        assert!(rate.is_finite() && rate > 0.0, "dephasing rate = {rate}");
        // At 20 mK with weak coupling the rate should be small (< 1 MHz)
        assert!(rate < 1.0, "dephasing rate {rate} MHz unexpectedly large");
    }

    #[test]
    fn phonon_dephasing_rate_zero_temp_is_zero() {
        // At T=0 the thermal dephasing vanishes (gamma ∝ T)
        let rate = phonon_dephasing_rate_mhz_inline(1e-4, 10.0, 0.0, 5.0);
        assert!((rate).abs() < 1e-30, "dephasing rate at T=0 should vanish, got {rate}");
    }

    #[test]
    fn phonon_dephasing_rate_scales_with_temperature() {
        // Dephasing rate is linear in T → doubling T doubles the rate
        let r1 = phonon_dephasing_rate_mhz_inline(1e-4, 10.0, 20.0, 5.0);
        let r2 = phonon_dephasing_rate_mhz_inline(1e-4, 10.0, 40.0, 5.0);
        assert!((r2 / r1 - 2.0).abs() < 1e-10, "rate should double: r1={r1}, r2={r2}");
    }

    // ── Phonon T1 ─────────────────────────────────────────────────────────

    #[test]
    fn phonon_t1_typical_transmon() {
        let t1 = phonon_t1_us_inline(1e-4, 10.0, 20.0, 5.0);
        assert!(t1.is_finite() && t1 > 0.0, "T1 = {t1}");
    }

    #[test]
    fn phonon_t1_coth_large_x_branch() {
        // At very low T the argument x = ℏω/(2kT) > 50, testing the coth≈1 branch
        let t1 = phonon_t1_us_inline(1e-4, 10.0, 0.01, 5.0);
        assert!(t1.is_finite() && t1 > 0.0, "T1 at very low T = {t1}");
    }

    #[test]
    fn phonon_t1_coth_normal_branch() {
        // At moderate T the coth uses the sinh/cosh path
        let t1 = phonon_t1_us_inline(1e-4, 10.0, 500.0, 5.0);
        assert!(t1.is_finite() && t1 > 0.0, "T1 at 500 mK = {t1}");
    }

    // ── Phonon T2 ─────────────────────────────────────────────────────────

    #[test]
    fn phonon_t2_bounded_by_2t1() {
        // T2 ≤ 2*T1 (fundamental quantum limit)
        let t1 = phonon_t1_us_inline(1e-4, 10.0, 20.0, 5.0);
        let t2 = phonon_t2_us_inline(1e-4, 10.0, 20.0, 5.0);
        assert!(t2 <= 2.0 * t1, "T2={t2} should be ≤ 2*T1={}", 2.0 * t1);
        assert!(t2 > 0.0, "T2 must be positive");
    }

    #[test]
    fn phonon_t2_decreases_with_temperature() {
        let t2_cold = phonon_t2_us_inline(1e-4, 10.0, 20.0, 5.0);
        let t2_warm = phonon_t2_us_inline(1e-4, 10.0, 200.0, 5.0);
        assert!(t2_cold > t2_warm, "T2 should decrease with temperature");
    }

    // ── Pkg export request serde roundtrip ────────────────────────────────

    #[test]
    fn pkg_export_request_serde_roundtrip() {
        let payload = json!({
            "assembly": {
                "housing": {
                    "internal_length_m": 0.040,
                    "internal_width_m": 0.030,
                    "internal_depth_m": 0.012,
                    "wall_thickness_m": 0.005,
                    "lid_thickness_m": 0.003,
                    "chip_recess_depth_m": 0.0005,
                    "chip_recess_length_m": 0.028,
                    "chip_recess_width_m": 0.021,
                    "material": "Aluminum6061"
                },
                "sma_connectors": [],
                "wirebonds": null,
                "indium_seal": null,
                "screw_pattern": null
            },
            "formats": ["gds", "stl"],
            "out_prefix": "/tmp/my_export"
        });
        let serialized = serde_json::to_string(&payload).unwrap();
        let back: Value = serde_json::from_str(&serialized).unwrap();
        let formats = back["formats"].as_array().unwrap();
        assert_eq!(formats.len(), 2);
        assert_eq!(formats[0].as_str().unwrap(), "gds");
        assert_eq!(formats[1].as_str().unwrap(), "stl");
        assert_eq!(back["out_prefix"].as_str().unwrap(), "/tmp/my_export");
        let il = back["assembly"]["housing"]["internal_length_m"].as_f64().unwrap();
        assert!((il - 0.040).abs() < 1e-9);
    }
}
