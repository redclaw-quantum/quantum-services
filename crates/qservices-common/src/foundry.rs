//! Foundry submission profiles.
//!
//! A [`FoundryProfile`] bundles the choices that make a tape-out package
//! targetable at a specific fabrication partner: which PDK DRC deck to check
//! against, the process string + minimum feature size for the OQFP fabrication
//! layer, whether DRC violations should block submission, and the required
//! bring-up test plan.
//!
//! These are **representative** profiles for superconducting coplanar processes,
//! not any specific foundry's certified/NDA specification. Shared by quantum-api
//! (the `/foundry/profiles` discovery endpoint) and the orchestration
//! `drc_check` / `tapeout_package` stages so there is a single source of truth.

use serde_json::{json, Value};

/// A named fabrication-partner submission profile.
#[derive(Debug, Clone, Copy)]
pub struct FoundryProfile {
    pub name: &'static str,
    pub description: &'static str,
    /// Name of the claw-gds DRC rule deck this foundry is checked against.
    pub pdk_deck: &'static str,
    /// Process string recorded in the OQFP fabrication layer.
    pub fab_process: &'static str,
    /// Minimum drawable feature size (nm).
    pub min_feature_nm: f64,
    /// Whether DRC violations should block the tape-out (gate the pipeline).
    pub gate_on_drc: bool,
    /// Required bring-up / characterization measurements.
    pub test_plan: &'static [&'static str],
}

impl FoundryProfile {
    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "pdk_deck": self.pdk_deck,
            "fab_process": self.fab_process,
            "min_feature_nm": self.min_feature_nm,
            "gate_on_drc": self.gate_on_drc,
            "test_plan": self.test_plan,
        })
    }
}

const STANDARD_TEST_PLAN: &[&str] = &[
    "resonator_spectroscopy",
    "qubit_spectroscopy",
    "T1",
    "T2_echo",
    "single_qubit_RB",
    "two_qubit_RB",
    "readout_fidelity",
];

const FOUNDRY_TEST_PLAN: &[&str] = &[
    "wafer_probe_continuity",
    "resonator_spectroscopy",
    "qubit_spectroscopy",
    "T1",
    "T2_echo",
    "single_qubit_RB",
    "two_qubit_RB",
    "readout_fidelity",
    "cross_resonance_calibration",
    "junction_resistance_map",
];

const PROFILES: &[FoundryProfile] = &[
    FoundryProfile {
        name: "university_snf",
        description: "Academic cleanroom, Al/AlOx coplanar process (lenient, report-only DRC).",
        pdk_deck: "coplanar_university",
        fab_process: "AlOx_0.5um",
        min_feature_nm: 500.0,
        gate_on_drc: false,
        test_plan: STANDARD_TEST_PLAN,
    },
    FoundryProfile {
        name: "commercial_foundry",
        description: "Commercial superconducting foundry, tighter rules, DRC-gated submission.",
        pdk_deck: "coplanar_foundry",
        fab_process: "AlOx_0.25um",
        min_feature_nm: 250.0,
        gate_on_drc: true,
        test_plan: FOUNDRY_TEST_PLAN,
    },
];

/// Look up a foundry profile by name. Returns `None` for unknown names.
pub fn profile(name: &str) -> Option<FoundryProfile> {
    PROFILES.iter().find(|p| p.name == name).copied()
}

/// Names of all built-in foundry profiles.
pub fn profile_names() -> Vec<&'static str> {
    PROFILES.iter().map(|p| p.name).collect()
}

/// All built-in foundry profiles serialized for discovery endpoints.
pub fn all_profiles_json() -> Value {
    json!({ "profiles": PROFILES.iter().map(|p| p.to_json()).collect::<Vec<_>>() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_profiles_resolve() {
        for name in profile_names() {
            assert!(profile(name).is_some(), "{name} must resolve");
        }
        assert!(profile("no_such_foundry").is_none());
    }

    #[test]
    fn profiles_reference_real_decks() {
        // Every profile's deck must be one claw-gds knows about. Kept in sync
        // by name; claw-gds owns the deck definitions.
        let known_decks = ["default", "coplanar_university", "coplanar_foundry"];
        for name in profile_names() {
            let p = profile(name).unwrap();
            assert!(
                known_decks.contains(&p.pdk_deck),
                "profile {name} references unknown deck {}",
                p.pdk_deck
            );
        }
    }

    #[test]
    fn commercial_foundry_gates_on_drc() {
        assert!(profile("commercial_foundry").unwrap().gate_on_drc);
        assert!(!profile("university_snf").unwrap().gate_on_drc);
    }

    #[test]
    fn json_round_trips_key_fields() {
        let j = profile("commercial_foundry").unwrap().to_json();
        assert_eq!(j["pdk_deck"], json!("coplanar_foundry"));
        assert_eq!(j["min_feature_nm"], json!(250.0));
        assert!(j["test_plan"].as_array().is_some_and(|a| !a.is_empty()));
    }
}
