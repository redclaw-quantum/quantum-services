use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldEstimate {
    pub yield_fraction: f64,
    pub sample_count: usize,
    pub collision_free_fraction: f64,
    pub mean_worst_collision_mhz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YieldReport {
    pub design_points: usize,
    pub mean_yield: f64,
    pub best_design_idx: usize,
    pub yield_estimates: Vec<YieldEstimate>,
    pub common_failure_modes: Vec<String>,
}
