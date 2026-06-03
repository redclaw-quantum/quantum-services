use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipLayout {
    pub qubits: Vec<ChipQubit>,
    pub couplers: Vec<ChipCoupler>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipQubit {
    pub id: usize,
    pub x_um: f64,
    pub y_um: f64,
    pub frequency_ghz: f64,
    pub anharmonicity_mhz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChipCoupler {
    pub qubit_a: usize,
    pub qubit_b: usize,
    pub coupler_type: String,
    pub strength_mhz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrequencyPlan {
    pub assignments: Vec<FreqAssignment>,
    pub collision_free: bool,
    pub worst_collision_mhz: f64,
    pub zz_map: Vec<Vec<f64>>,
    pub yield_estimate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FreqAssignment {
    pub qubit_id: usize,
    pub frequency_ghz: f64,
    pub slot_index: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrosstalkMap {
    pub coupling_matrix_mhz: Vec<Vec<f64>>,
    pub zz_static_khz: Vec<Vec<f64>>,
    pub microwave_leakage: Vec<MicrowaveLeakage>,
    pub worst_case_gate_fidelity: f64,
    pub max_zz_khz: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MicrowaveLeakage {
    pub drive_qubit: usize,
    pub leakage_qubit: usize,
    pub epsilon: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadoutParams {
    pub resonator_frequency_ghz: f64,
    pub coupling_mhz: f64,
    pub kappa_mhz: f64,
    pub chi_mhz: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chip_qubit_serde_roundtrip() {
        let q = ChipQubit {
            id: 3,
            x_um: 100.0,
            y_um: 200.0,
            frequency_ghz: 5.1,
            anharmonicity_mhz: -330.0,
        };
        let json = serde_json::to_string(&q).unwrap();
        let q2: ChipQubit = serde_json::from_str(&json).unwrap();
        assert_eq!(q2.id, 3);
        assert!((q2.frequency_ghz - 5.1).abs() < 1e-9);
        assert!((q2.anharmonicity_mhz - (-330.0)).abs() < 1e-9);
    }

    #[test]
    fn chip_layout_serde_roundtrip() {
        let layout = ChipLayout {
            qubits: vec![
                ChipQubit { id: 0, x_um: 0.0, y_um: 0.0, frequency_ghz: 5.0, anharmonicity_mhz: -320.0 },
                ChipQubit { id: 1, x_um: 500.0, y_um: 0.0, frequency_ghz: 5.2, anharmonicity_mhz: -325.0 },
            ],
            couplers: vec![
                ChipCoupler { qubit_a: 0, qubit_b: 1, coupler_type: "tunable".to_string(), strength_mhz: 10.0 },
            ],
        };
        let json = serde_json::to_string(&layout).unwrap();
        let layout2: ChipLayout = serde_json::from_str(&json).unwrap();
        assert_eq!(layout2.qubits.len(), 2);
        assert_eq!(layout2.couplers.len(), 1);
        assert_eq!(layout2.couplers[0].coupler_type, "tunable");
        assert!((layout2.couplers[0].strength_mhz - 10.0).abs() < 1e-9);
    }

    #[test]
    fn frequency_plan_collision_free_flag() {
        let plan = FrequencyPlan {
            assignments: vec![
                FreqAssignment { qubit_id: 0, frequency_ghz: 5.0, slot_index: 0 },
                FreqAssignment { qubit_id: 1, frequency_ghz: 5.2, slot_index: 1 },
            ],
            collision_free: true,
            worst_collision_mhz: 200.0,
            zz_map: vec![vec![0.0, -50.0], vec![-50.0, 0.0]],
            yield_estimate: 0.95,
        };
        let json = serde_json::to_string(&plan).unwrap();
        let plan2: FrequencyPlan = serde_json::from_str(&json).unwrap();
        assert!(plan2.collision_free);
        assert_eq!(plan2.assignments.len(), 2);
        assert!((plan2.yield_estimate - 0.95).abs() < 1e-9);
    }

    #[test]
    fn crosstalk_map_serde_roundtrip() {
        let cm = CrosstalkMap {
            coupling_matrix_mhz: vec![vec![0.0, 5.0], vec![5.0, 0.0]],
            zz_static_khz: vec![vec![0.0, -30.0], vec![-30.0, 0.0]],
            microwave_leakage: vec![MicrowaveLeakage {
                drive_qubit: 0,
                leakage_qubit: 1,
                epsilon: 0.01,
            }],
            worst_case_gate_fidelity: 0.998,
            max_zz_khz: 30.0,
        };
        let json = serde_json::to_string(&cm).unwrap();
        let cm2: CrosstalkMap = serde_json::from_str(&json).unwrap();
        assert_eq!(cm2.microwave_leakage.len(), 1);
        assert!((cm2.max_zz_khz - 30.0).abs() < 1e-9);
        assert!((cm2.worst_case_gate_fidelity - 0.998).abs() < 1e-9);
    }

    #[test]
    fn readout_params_fields_preserved() {
        let rp = ReadoutParams {
            resonator_frequency_ghz: 6.8,
            coupling_mhz: 50.0,
            kappa_mhz: 1.0,
            chi_mhz: -0.5,
        };
        let json = serde_json::to_string(&rp).unwrap();
        let rp2: ReadoutParams = serde_json::from_str(&json).unwrap();
        assert!((rp2.resonator_frequency_ghz - 6.8).abs() < 1e-9);
        assert!((rp2.chi_mhz - (-0.5)).abs() < 1e-9);
    }
}
