use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HamiltonianTarget {
    pub device_type: DeviceType,
    pub qubit_frequency_ghz: f64,
    pub anharmonicity_mhz: f64,
    pub coupling_mhz: f64,
    pub linewidth_khz: Option<f64>,
    pub n_candidates: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HamiltonianParams {
    pub qubit_freq_ghz: f64,
    pub anharmonicity_mhz: f64,
    pub coupling_mhz: f64,
    pub linewidth_khz: f64,
    pub t1_estimate_us: Option<f64>,
    pub source: HamiltonianSource,
    pub geometry: Vec<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceType {
    TransmonCross,
    TunableTransmon,
    CavityResonator,
    StarmonHex,
    FluxoniumDimer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HamiltonianSource {
    Analytical,
    Electrostatic,
    Eigenmode,
}
