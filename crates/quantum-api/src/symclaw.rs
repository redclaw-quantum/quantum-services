//! SymClaw symbolic math API handlers.
//! Extracted from main.rs to keep file within 1300-line limit.


use axum::Json;
use serde_json::{Value, json};

use super::{ApiResult, run_symclaw};

pub async fn symclaw_health() -> Json<Value> {
    Json(json!({"status": "ok", "tool": "symclaw-skill", "actions": [
        "simplify", "differentiate", "integrate", "solve", "taylor",
        "limit", "codegen", "linalg", "polynomial", "analyze"
    ]}))
}

/// POST /symclaw/simplify — simplify a symbolic expression.
///
/// Accepts: `{ "expr": "x^2 + 2*x + 1" }`.
pub async fn symclaw_simplify(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let result = run_symclaw(&json!({"action": "simplify", "expr": expr}))?;
    Ok(Json(result))
}

/// POST /symclaw/differentiate — differentiate an expression.
///
/// Accepts: `{ "expr": "sin(x^2)", "var": "x", "order": 1 }`.
pub async fn symclaw_differentiate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let order = req.get("order").and_then(|v| v.as_u64()).unwrap_or(1);
    let result = run_symclaw(&json!({"action": "differentiate", "expr": expr, "var": var, "order": order}))?;
    Ok(Json(result))
}

/// POST /symclaw/integrate — integrate an expression.
///
/// Accepts: `{ "expr": "x^2", "var": "x", "lower": "0", "upper": "1" }`.
/// Omit lower/upper for indefinite integral.
pub async fn symclaw_integrate(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let mut request = json!({"action": "integrate", "expr": expr, "var": var});
    if let Some(lo) = req.get("lower").and_then(|v| v.as_str()) {
        request["lower"] = json!(lo);
    }
    if let Some(hi) = req.get("upper").and_then(|v| v.as_str()) {
        request["upper"] = json!(hi);
    }
    let result = run_symclaw(&request)?;
    Ok(Json(result))
}

/// POST /symclaw/solve — solve an equation for a variable.
///
/// Accepts: `{ "expr": "x^2 - 4", "var": "x" }`.
pub async fn symclaw_solve(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let result = run_symclaw(&json!({"action": "solve", "expr": expr, "var": var}))?;
    Ok(Json(result))
}

/// POST /symclaw/taylor — Taylor series expansion.
///
/// Accepts: `{ "expr": "exp(x)", "var": "x", "order": 4, "point": "0" }`.
pub async fn symclaw_taylor(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let order = req.get("order").and_then(|v| v.as_u64()).unwrap_or(4);
    let mut request = json!({"action": "taylor", "expr": expr, "var": var, "order": order});
    if let Some(pt) = req.get("point").and_then(|v| v.as_str()) {
        request["point"] = json!(pt);
    }
    let result = run_symclaw(&request)?;
    Ok(Json(result))
}

/// POST /symclaw/limit — compute a symbolic limit.
///
/// Accepts: `{ "expr": "sin(x)/x", "var": "x", "point": "0" }`.
pub async fn symclaw_limit(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let point = req.get("point").and_then(|v| v.as_str()).unwrap_or("0");
    let result = run_symclaw(&json!({"action": "limit", "expr": expr, "var": var, "value": point}))?;
    Ok(Json(result))
}

/// POST /symclaw/codegen — generate code from a symbolic expression.
///
/// Accepts: `{ "expr": "x^2 + 2*x + 1", "lang": "python" }`.
/// Languages: python, c, rust, julia, js, glsl, wgsl.
pub async fn symclaw_codegen(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let lang = req.get("lang").and_then(|v| v.as_str()).unwrap_or("python");
    let result = run_symclaw(&json!({"action": "codegen", "expr": expr, "language": lang}))?;
    Ok(Json(result))
}

/// POST /symclaw/linalg — linear algebra operations.
///
/// Accepts: `{ "op": "det"|"inv"|"eigenvalues"|"solve", "matrix": [[...]], "vector": [...] }`.
pub async fn symclaw_linalg(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let op = req.get("op").and_then(|v| v.as_str()).unwrap_or("det");
    let matrix = req.get("matrix")
        .ok_or_else(|| anyhow::anyhow!("missing field: matrix"))?;
    let mut request = json!({"action": "linalg", "command": op, "matrix": matrix});
    if let Some(v) = req.get("vector") {
        request["vector"] = v.clone();
    }
    let result = run_symclaw(&request)?;
    Ok(Json(result))
}

/// POST /symclaw/polynomial — polynomial operations.
///
/// Accepts: `{ "op": "gcd"|"factor"|"roots"|"expand", "poly": "x^2 - 1", "var": "x" }`.
pub async fn symclaw_polynomial(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let _op = req.get("op").and_then(|v| v.as_str()).unwrap_or("factor");
    let poly = req.get("poly").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: poly"))?;
    let _var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let result = run_symclaw(&json!({"action": "polynomial", "expr": poly}))?;
    Ok(Json(result))
}

/// POST /symclaw/analyze — full symbolic analysis of an expression.
///
/// Accepts: `{ "expr": "x^3 - x", "var": "x" }`.
/// Returns: zeros, critical points, inflections, LaTeX, and series expansion.
pub async fn symclaw_analyze(Json(req): Json<Value>) -> ApiResult<Json<Value>> {
    let expr = req.get("expr").and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("missing field: expr"))?;
    let var = req.get("var").and_then(|v| v.as_str()).unwrap_or("x");
    let result = run_symclaw(&json!({"action": "analyze", "expr": expr, "var": var}))?;
    Ok(Json(result))
}
