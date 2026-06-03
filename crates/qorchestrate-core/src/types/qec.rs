use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QecAssessment {
    pub meets_threshold: bool,
    pub margin_db: f64,
    pub logical_error_rate: f64,
    pub max_achievable_distance: usize,
    pub physical_overhead: f64,
    pub recommended_code: String,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QvClops {
    pub quantum_volume: u64,
    pub clops: f64,
    pub gate_fidelity_2q: f64,
}
