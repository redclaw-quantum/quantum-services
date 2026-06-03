use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizedPulse {
    pub gate: String,
    pub fidelity: f64,
    pub duration_ns: f64,
    pub pulse_shape: PulseShape,
    pub amplitudes: Vec<f64>,
    pub iterations_used: usize,
    pub converged: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PulseShape {
    Grape,
    Drag,
    Rl,
    Gaussian,
    Square,
}
