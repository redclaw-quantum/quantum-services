use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviationReport {
    pub deviations: Vec<Deviation>,
    pub health_status: HealthStatus,
    pub health_score: f64,
    pub critical_count: usize,
    pub failed_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deviation {
    pub qubit_id: usize,
    pub parameter: String,
    pub expected: f64,
    pub measured: f64,
    pub delta: f64,
    pub severity: Severity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum HealthStatus {
    Excellent,
    Good,
    Marginal,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum Severity {
    Low,
    Medium,
    High,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalActions {
    pub actions: Vec<RecalAction>,
    pub critical_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecalAction {
    pub qubit_id: usize,
    pub action_type: String,
    pub description: String,
    pub new_geometry: Option<Vec<f64>>,
}
