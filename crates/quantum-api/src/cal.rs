//! Calibration (rustycal) API handlers.
//! Extracted from main.rs to keep file within 1300-line limit.


use axum::Json;
use serde_json::{Value, json};

use super::{ApiResult, run_tool};

// rustycal endpoints
// ---------------------------------------------------------------------------

/// GET /cal/health
pub async fn cal_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "rustycal"}))
}

/// POST /cal/spectroscopy — simulate qubit spectroscopy and fit Lorentzian.
///
/// Accepts: `{ "freq_start": float, "freq_stop": float, "points": int }`.
pub async fn cal_spectroscopy(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let freq_start_s = req.get("freq_start").and_then(|v| v.as_f64()).unwrap_or(4.5).to_string();
    let freq_stop_s = req.get("freq_stop").and_then(|v| v.as_f64()).unwrap_or(5.5).to_string();
    let points_s = req.get("points").and_then(|v| v.as_u64()).unwrap_or(100).to_string();
    let result = run_tool("rustycal", &[
        "--json", "spectroscopy",
        "--freq-start", &freq_start_s,
        "--freq-stop", &freq_stop_s,
        "--points", &points_s,
    ])?;
    Ok(Json(result))
}

/// POST /cal/rabi — simulate Rabi oscillations and extract π-pulse amplitude.
///
/// Accepts: `{ "amp_start": float, "amp_stop": float, "points": int }`.
pub async fn cal_rabi(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let amp_start_s = req.get("amp_start").and_then(|v| v.as_f64()).unwrap_or(0.0).to_string();
    let amp_stop_s = req.get("amp_stop").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let points_s = req.get("points").and_then(|v| v.as_u64()).unwrap_or(50).to_string();
    let result = run_tool("rustycal", &[
        "--json", "rabi",
        "--amp-start", &amp_start_s,
        "--amp-stop", &amp_stop_s,
        "--points", &points_s,
    ])?;
    Ok(Json(result))
}

/// POST /cal/t1 — simulate T1 decay and extract relaxation time.
///
/// Accepts: `{ "max_delay": float, "points": int }`.
pub async fn cal_t1(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let max_delay_s = req.get("max_delay").and_then(|v| v.as_f64()).unwrap_or(100.0).to_string();
    let points_s = req.get("points").and_then(|v| v.as_u64()).unwrap_or(100).to_string();
    let result = run_tool("rustycal", &[
        "--json", "t1",
        "--max-delay", &max_delay_s,
        "--points", &points_s,
    ])?;
    Ok(Json(result))
}

/// POST /cal/rb — simulate randomized benchmarking and extract error per Clifford.
///
/// Accepts: `{ "max_cliffords": int, "sequences": int, "points": int }`.
pub async fn cal_rb(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let max_cliffords_s = req.get("max_cliffords").and_then(|v| v.as_u64()).unwrap_or(200).to_string();
    let sequences_s = req.get("sequences").and_then(|v| v.as_u64()).unwrap_or(50).to_string();
    let points_s = req.get("points").and_then(|v| v.as_u64()).unwrap_or(20).to_string();
    let result = run_tool("rustycal", &[
        "--json", "rb",
        "--max-cliffords", &max_cliffords_s,
        "--sequences", &sequences_s,
        "--points", &points_s,
    ])?;
    Ok(Json(result))
}

/// POST /cal/cycle-rb — multi-layer cycle randomized benchmarking (SPL noise model).
///
/// Accepts: `{ "max_depth": int, "sequences": int, "points": int, "layers": int }`.
pub async fn cal_cycle_rb(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let max_depth_s = req.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(100).to_string();
    let sequences_s = req.get("sequences").and_then(|v| v.as_u64()).unwrap_or(30).to_string();
    let points_s = req.get("points").and_then(|v| v.as_u64()).unwrap_or(15).to_string();
    let layers_s = req.get("layers").and_then(|v| v.as_u64()).unwrap_or(2).to_string();
    let result = run_tool("rustycal", &[
        "--json",
        "cycle-rb",
        "--max-depth", &max_depth_s,
        "--sequences", &sequences_s,
        "--points", &points_s,
        "--layers", &layers_s,
    ])?;
    Ok(Json(result))
}

/// POST /cal/adaptive — Bayesian adaptive Ramsey recalibration.
///
/// Accepts: `{ "freq_ghz": float, "sigma_mhz": float, "t_ramsey_ns": float,
///            "max_shots": int, "threshold_mhz": float }`.
pub async fn cal_adaptive(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let freq_s = req.get("freq_ghz").and_then(|v| v.as_f64()).unwrap_or(5.0).to_string();
    let sigma_s = req.get("sigma_mhz").and_then(|v| v.as_f64()).unwrap_or(1.0).to_string();
    let t_ramsey_s = req.get("t_ramsey_ns").and_then(|v| v.as_f64()).unwrap_or(500.0).to_string();
    let max_shots_s = req.get("max_shots").and_then(|v| v.as_u64()).unwrap_or(50).to_string();
    let threshold_s = req.get("threshold_mhz").and_then(|v| v.as_f64()).unwrap_or(0.05).to_string();
    let result = run_tool("rustycal", &[
        "--json",
        "adaptive-cal",
        "--freq-ghz", &freq_s,
        "--sigma-mhz", &sigma_s,
        "--t-ramsey-ns", &t_ramsey_s,
        "--max-shots", &max_shots_s,
        "--threshold-mhz", &threshold_s,
    ])?;
    Ok(Json(result))
}

pub async fn cal_leakage_rb(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let max_depth = req.get("max_depth").and_then(|v| v.as_u64()).unwrap_or(200);
    let sequences = req.get("sequences").and_then(|v| v.as_u64()).unwrap_or(30);
    let points = req.get("points").and_then(|v| v.as_u64()).unwrap_or(10);
    let leakage_rate = req.get("leakage_rate").and_then(|v| v.as_f64()).unwrap_or(0.001);
    let seepage_rate = req.get("seepage_rate").and_then(|v| v.as_f64()).unwrap_or(0.002);
    let epc = req.get("epc").and_then(|v| v.as_f64()).unwrap_or(0.002);
    let arg_max_depth = format!("--max-depth={max_depth}");
    let arg_sequences = format!("--sequences={sequences}");
    let arg_points = format!("--points={points}");
    let arg_leakage_rate = format!("--leakage-rate={leakage_rate}");
    let arg_seepage_rate = format!("--seepage-rate={seepage_rate}");
    let arg_epc = format!("--epc={epc}");
    let result = run_tool("rustycal", &[
        "leakage-rb", "--json",
        &arg_max_depth, &arg_sequences, &arg_points,
        &arg_leakage_rate, &arg_seepage_rate, &arg_epc,
    ])?;
    Ok(Json(result))
}
