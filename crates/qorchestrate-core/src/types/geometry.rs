use serde::{Deserialize, Serialize};

use crate::types::hamiltonian::DeviceType;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryCandidates {
    pub candidates: Vec<GeometryCandidate>,
    pub best_candidate_idx: usize,
    pub device_type: DeviceType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometryCandidate {
    pub geometry: Vec<f64>,
    pub predicted_freq_ghz: f64,
    pub predicted_anhar_mhz: f64,
    pub uncertainty_std: f64,
    pub is_valid: bool,
    pub source: DesignSource,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesignSource {
    Rmflow,
    Cmaes,
    QemOracle,
}
