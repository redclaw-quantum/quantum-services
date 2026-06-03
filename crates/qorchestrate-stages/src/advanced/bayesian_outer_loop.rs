use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use nalgebra::{DMatrix, DVector};
use serde_json::{json, Value};

use qorchestrate_core::{
    errors::StageError,
    pipeline::PipelineDef,
    stage::{Stage, StageContext, StageType},
};
use qorchestrate_executor::PipelineExecutor;

pub struct BayesianOuterLoopStage {
    /// `Weak` breaks the `Registry → Stage → Executor → Registry` cycle.
    /// The executor is constructed via `Arc::new_cyclic` so this weak ref
    /// is valid for the executor's full lifetime.
    executor: Weak<PipelineExecutor>,
    templates_dir: PathBuf,
}

impl BayesianOuterLoopStage {
    pub fn new(executor: Weak<PipelineExecutor>, templates_dir: PathBuf) -> Self {
        Self {
            executor,
            templates_dir,
        }
    }
}

// ── Pure GP / acquisition helpers (free functions, no captures) ──────────────

fn random_unit(n_dims: usize, salt: u64) -> Vec<f64> {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .hash(&mut hasher);
    salt.hash(&mut hasher);
    let seed = hasher.finish();
    (0..n_dims)
        .map(|i| {
            let mut h = DefaultHasher::new();
            (seed ^ (i as u64).wrapping_mul(6_364_136_223_846_793_005)).hash(&mut h);
            (h.finish() as f64) / (u64::MAX as f64)
        })
        .collect()
}

/// One dimension of the Bayesian search space.
///
/// `Continuous` maps the GP's [0, 1] unit value linearly into `[low, high]`.
/// `Categorical` maps it by index into the `choices` list — letting the
/// optimizer pick between species, platforms, layouts, etc. The GP itself
/// still treats every dimension as continuous; the mapping at injection
/// time turns the unit value into the categorical pick.
enum SearchDim {
    Continuous { param: String, low: f64, high: f64 },
    Categorical { param: String, choices: Vec<Value> },
}

fn unit_to_params(unit: &[f64], search_space: &[SearchDim]) -> Value {
    let mut map = serde_json::Map::new();
    for (i, dim) in search_space.iter().enumerate() {
        match dim {
            SearchDim::Continuous { param, low, high } => {
                let v = low + unit[i] * (high - low);
                map.insert(param.clone(), json!(v));
            }
            SearchDim::Categorical { param, choices } if !choices.is_empty() => {
                let idx = ((unit[i] * choices.len() as f64) as usize).min(choices.len() - 1);
                map.insert(param.clone(), choices[idx].clone());
            }
            SearchDim::Categorical { .. } => {} // empty `choices`: skip
        }
    }
    Value::Object(map)
}

fn rbf_kernel(x1: &[f64], x2: &[f64], length_scale: f64) -> f64 {
    let sq_dist: f64 = x1.iter().zip(x2.iter()).map(|(a, b)| (a - b).powi(2)).sum();
    (-sq_dist / (2.0 * length_scale.powi(2))).exp()
}

fn gp_predict(
    x_train: &[Vec<f64>],
    y_train: &[f64],
    x_new: &[f64],
    noise: f64,
    length_scale: f64,
) -> (f64, f64) {
    let n = x_train.len();
    if n == 0 {
        return (0.0, 1.0);
    }

    let mut k_mat = DMatrix::zeros(n, n);
    for i in 0..n {
        for j in 0..n {
            k_mat[(i, j)] = rbf_kernel(&x_train[i], &x_train[j], length_scale);
        }
        k_mat[(i, i)] += noise;
    }

    let k_star: DVector<f64> = DVector::from_iterator(
        n,
        x_train.iter().map(|x| rbf_kernel(x, x_new, length_scale)),
    );

    let y_vec = DVector::from_vec(y_train.to_vec());

    let alpha = match k_mat.clone().cholesky() {
        Some(chol) => chol.solve(&y_vec),
        None => k_mat
            .clone()
            .try_inverse()
            .map(|ki| ki * &y_vec)
            .unwrap_or(y_vec.clone()),
    };

    let mu = k_star.dot(&alpha);

    let v = match k_mat.clone().cholesky() {
        Some(chol) => chol.solve(&k_star),
        None => k_mat
            .try_inverse()
            .map(|ki| ki * &k_star)
            .unwrap_or(k_star.clone()),
    };

    let k_ss = rbf_kernel(x_new, x_new, length_scale);
    let variance = (k_ss - k_star.dot(&v)).max(1e-8);
    (mu, variance.sqrt())
}

/// Abramowitz & Stegun erf approximation (max error < 1.5e-7).
fn approx_erf(x: f64) -> f64 {
    let t = 1.0 / (1.0 + 0.327_591_1 * x.abs());
    let poly = t * (0.254_829_592
        + t * (-0.284_496_736
            + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let result = 1.0 - poly * (-x * x).exp();
    if x >= 0.0 { result } else { -result }
}

fn expected_improvement(mu: f64, sigma: f64, y_best: f64) -> f64 {
    if sigma < 1e-10 {
        return 0.0;
    }
    let z = (y_best - mu) / sigma;
    let phi_z = (-z * z / 2.0).exp() / (2.0 * std::f64::consts::PI).sqrt();
    let big_phi_z = 0.5 * (1.0 + approx_erf(z / std::f64::consts::SQRT_2));
    (y_best - mu) * big_phi_z + sigma * phi_z
}

/// Walk a dot-separated path through a JSON value and return f64 if found.
fn extract_field_value(v: &Value, path: &str) -> Option<f64> {
    let mut current = v;
    for part in path.split('.') {
        current = current.get(part)?;
    }
    current.as_f64()
}

// ─────────────────────────────────────────────────────────────────────────────

#[async_trait]
impl Stage for BayesianOuterLoopStage {
    fn stage_type(&self) -> StageType {
        StageType::BayesianOuterLoop
    }

    fn timeout_secs(&self) -> u64 {
        7200
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let inner_template = input
            .get("inner_template")
            .and_then(|v| v.as_str())
            .unwrap_or("design-to-chip")
            .to_string();

        let objective_field = input
            .get("objective_field")
            .and_then(|v| v.as_str())
            .unwrap_or("logical_error_rate")
            .to_string();

        // Direction defaults to minimize for backward compatibility with the
        // original active-design-loop template, which targets logical-error
        // rate. Templates that want to maximize fidelity, yield, or QV pass
        // `objective_direction = "maximize"` in their params block.
        let maximize = input
            .get("objective_direction")
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case("maximize") || s.eq_ignore_ascii_case("max"))
            .unwrap_or(false);

        let n_iterations = input
            .get("n_iterations")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        let n_initial = input
            .get("n_initial_random")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as usize;

        // Each entry is either {param, low, high} (continuous) or
        // {param, choices: [...]} (categorical). Categorical entries let the
        // optimizer pick over species / platforms / layouts.
        let search_space: Vec<SearchDim> = input
            .get("search_space")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        let param = item.get("param")?.as_str()?.to_string();
                        if let Some(choices) = item.get("choices").and_then(|v| v.as_array()) {
                            Some(SearchDim::Categorical { param, choices: choices.clone() })
                        } else {
                            let low = item.get("low")?.as_f64()?;
                            let high = item.get("high")?.as_f64()?;
                            Some(SearchDim::Continuous { param, low, high })
                        }
                    })
                    .collect()
            })
            .unwrap_or_default();

        if search_space.is_empty() {
            return Err(StageError::InvalidInput(
                "bayesian_outer_loop requires non-empty 'search_space'".to_string(),
            ));
        }

        let n_dims = search_space.len();

        let toml_path = self
            .templates_dir
            .join(format!("{}.toml", inner_template));
        let toml_str = std::fs::read_to_string(&toml_path).map_err(|e| {
            StageError::InvalidInput(format!("template '{}' not found: {}", inner_template, e))
        })?;

        let def = Arc::new(PipelineDef::from_toml(&toml_str).map_err(|e| {
            StageError::InvalidInput(format!("template parse error: {}", e))
        })?);

        let brain_path = ctx.brain_path.to_string_lossy().to_string();

        // The GP / EI machinery internally minimizes. For a `maximize`
        // objective we negate the observed value on the way in and negate
        // again on the way out, so the user sees raw values throughout.
        let to_internal = |y: f64| if maximize { -y } else { y };
        let from_internal = |y: f64| if maximize { -y } else { y };

        let missing_y_internal = f64::INFINITY; // worst possible under minimization

        let mut x_obs: Vec<Vec<f64>> = Vec::new();
        let mut y_obs: Vec<f64> = Vec::new();
        let mut best_params: Value = Value::Null;
        let mut best_y_internal = f64::INFINITY;
        let mut best_output: Value = Value::Null;
        let mut all_iterations = Vec::new();

        for iter in 0..(n_initial + n_iterations) {
            let candidate_unit: Vec<f64> = if iter < n_initial {
                random_unit(n_dims, iter as u64)
            } else {
                // Sample 50 random candidates and pick the one with highest EI.
                let candidates: Vec<Vec<f64>> = (0..50u64)
                    .map(|salt| random_unit(n_dims, iter as u64 ^ salt.wrapping_mul(31)))
                    .collect();

                candidates
                    .into_iter()
                    .map(|c| {
                        let (mu, sigma) = gp_predict(&x_obs, &y_obs, &c, 1e-3, 1.0);
                        let ei = expected_improvement(mu, sigma, best_y_internal);
                        (c, ei)
                    })
                    .max_by(|a, b| {
                        a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                    })
                    .map(|(c, _)| c)
                    .unwrap_or_else(|| random_unit(n_dims, iter as u64))
            };

            let candidate_params = unit_to_params(&candidate_unit, &search_space);

            let mut run_params = input.clone();
            if let (Value::Object(m), Value::Object(cand)) =
                (&mut run_params, &candidate_params)
            {
                m.extend(cand.clone());
            }

            let executor = self.executor.upgrade().ok_or_else(|| {
                StageError::BackendError(
                    "PipelineExecutor dropped before bayesian iteration".into(),
                )
            })?;
            let run_result = executor
                .run_pipeline(&def, run_params, &brain_path)
                .await;

            let y_observed = match &run_result {
                Ok(state) => {
                    let output = state.output.as_ref().cloned().unwrap_or(Value::Null);
                    extract_field_value(&output, &objective_field)
                        .map(to_internal)
                        .unwrap_or(missing_y_internal)
                }
                Err(_) => missing_y_internal,
            };

            let iter_output = run_result
                .ok()
                .and_then(|s| s.output)
                .unwrap_or(Value::Null);

            all_iterations.push(json!({
                "iteration": iter,
                "params": candidate_params,
                "objective": from_internal(y_observed),
                "phase": if iter < n_initial { "exploration" } else { "exploitation" }
            }));

            if y_observed < best_y_internal {
                best_y_internal = y_observed;
                best_params = candidate_params.clone();
                best_output = iter_output;
            }

            x_obs.push(candidate_unit);
            y_obs.push(y_observed);
        }

        Ok(json!({
            "best_params": best_params,
            "best_objective": from_internal(best_y_internal),
            "best_output": best_output,
            "n_iterations": n_initial + n_iterations,
            "iterations": all_iterations,
            "objective_field": objective_field,
            "objective_direction": if maximize { "maximize" } else { "minimize" },
        }))
    }
}
