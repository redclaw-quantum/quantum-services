use std::fmt::Write as _;
use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

/// Assemble the fabrication-handoff bundle for a completed design.
///
/// Reads the GDS-II bytes (`gds_generate`), DRC report (`drc_check`) and the
/// validated OQFP spec (`oqfp_build` / `oqfp_validate`) from upstream and writes
/// a self-contained submission directory:
///
/// ```text
/// <base>/<run_id>/
///   chip.gds
///   drc_report.json
///   oqfp_spec.json
///   manifest.json   (sha256 of every file, fab profile, counts, test plan)
/// ```
///
/// The base directory is `$QORCH_OUTPUT_DIR` if set, otherwise
/// `<temp>/quantumclaw-tapeout`. The stage output echoes the submission path,
/// the file list, and the manifest so it can serve as the pipeline's terminal
/// artifact.
pub struct TapeoutPackageStage;

impl TapeoutPackageStage {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TapeoutPackageStage {
    fn default() -> Self {
        Self::new()
    }
}

/// Default bring-up measurement plan shipped with every package, so the fab /
/// test partner knows the intended characterization sequence.
fn default_test_plan() -> Value {
    json!([
        "resonator_spectroscopy",
        "qubit_spectroscopy",
        "T1",
        "T2_echo",
        "single_qubit_RB",
        "two_qubit_RB",
        "readout_fidelity"
    ])
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Decode a lowercase/uppercase hex string into bytes. Returns `None` on any
/// non-hex character or odd length.
fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Resolve the base output directory for tape-out bundles. Precedence:
/// explicit `output_dir` in the stage input, then `$QORCH_OUTPUT_DIR`, then
/// `<temp>/quantumclaw-tapeout`.
fn base_output_dir(input: &Value) -> PathBuf {
    if let Some(d) = input.get("output_dir").and_then(|v| v.as_str())
        && !d.is_empty()
    {
        return PathBuf::from(d);
    }
    match std::env::var("QORCH_OUTPUT_DIR") {
        Ok(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::temp_dir().join("quantumclaw-tapeout"),
    }
}

#[async_trait]
impl Stage for TapeoutPackageStage {
    fn stage_type(&self) -> StageType {
        StageType::TapeoutPackage
    }

    fn timeout_secs(&self) -> u64 {
        30
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let gds = input
            .get("gds_generate_output")
            .cloned()
            .unwrap_or(Value::Null);
        let drc = input
            .get("drc_check_output")
            .cloned()
            .unwrap_or(Value::Null);

        // The OQFP spec itself lives in the build output; oqfp_validate carries
        // the validated flag.
        let oqfp_build = input
            .get("oqfp_build_output")
            .cloned()
            .unwrap_or(Value::Null);
        let oqfp_validate = input
            .get("oqfp_validate_output")
            .cloned()
            .unwrap_or(Value::Null);
        let oqfp_spec = oqfp_build.get("oqfp_spec").cloned().unwrap_or(Value::Null);
        let recipe_out = input
            .get("process_recipe_output")
            .cloned()
            .unwrap_or(Value::Null);
        let validated = oqfp_validate
            .get("validated")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // A tape-out package without geometry is meaningless.
        let gds_hex = gds
            .get("hex")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                StageError::InvalidInput(
                    "tapeout_package requires gds_generate_output.hex".to_string(),
                )
            })?;
        let gds_bytes = decode_hex(gds_hex)
            .ok_or_else(|| StageError::InvalidInput("gds hex is not valid hex".to_string()))?;

        // An optional foundry profile supplies the process string and test
        // plan; an explicit fab_process still wins.
        let foundry = input
            .get("foundry")
            .and_then(|v| v.as_str())
            .and_then(qservices_common::foundry::profile);
        let fab_process = input
            .get("fab_process")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .or_else(|| foundry.map(|f| f.fab_process.to_string()))
            .unwrap_or_else(|| "AlOx_0.5um".to_string());
        let test_plan = match foundry {
            Some(f) => json!(f.test_plan),
            None => default_test_plan(),
        };

        // Write the submission directory.
        let dir = base_output_dir(&input).join(ctx.pipeline_run_id.to_string());
        std::fs::create_dir_all(&dir)
            .map_err(|e| StageError::BackendError(format!("create submission dir: {e}")))?;

        let drc_bytes = serde_json::to_vec_pretty(&drc)?;
        let spec_bytes = serde_json::to_vec_pretty(&oqfp_spec)?;

        let write = |name: &str, bytes: &[u8]| -> Result<(), StageError> {
            std::fs::write(dir.join(name), bytes)
                .map_err(|e| StageError::BackendError(format!("write {name}: {e}")))
        };
        write("chip.gds", &gds_bytes)?;
        write("drc_report.json", &drc_bytes)?;
        write("oqfp_spec.json", &spec_bytes)?;

        let manifest = json!({
            "package_format_version": "1.0",
            "run_id": ctx.pipeline_run_id.to_string(),
            "created_utc": chrono::Utc::now().to_rfc3339(),
            "foundry": foundry.map(|f| f.name).unwrap_or("none"),
            "fab_process": fab_process,
            "min_feature_nm": foundry.map(|f| f.min_feature_nm),
            "chip": {
                "num_qubits": gds.get("num_qubits").cloned().unwrap_or(Value::Null),
                "num_resonators": gds.get("num_resonators").cloned().unwrap_or(Value::Null),
                "num_bus_couplers": gds.get("num_bus_couplers").cloned().unwrap_or(Value::Null),
            },
            "files": {
                "chip.gds": {
                    "n_bytes": gds_bytes.len(),
                    "sha256": sha256_hex(&gds_bytes),
                    "format": "gds2",
                },
                "drc_report.json": {
                    "n_bytes": drc_bytes.len(),
                    "sha256": sha256_hex(&drc_bytes),
                    "deck": drc.get("deck").cloned().unwrap_or(Value::Null),
                    "clean": drc.get("clean").cloned().unwrap_or(Value::Null),
                    "num_violations": drc.get("num_violations").cloned().unwrap_or(Value::Null),
                },
                "oqfp_spec.json": {
                    "n_bytes": spec_bytes.len(),
                    "sha256": sha256_hex(&spec_bytes),
                    "validated": validated,
                },
            },
            "process": {
                "junction_recipe": recipe_out.get("recipe").and_then(|r| r.get("name")).cloned().unwrap_or(Value::Null),
                "junction": recipe_out.get("eval").cloned().unwrap_or(Value::Null),
            },
            "tapeout_deck": {
                "layer_map": gds.get("layer_map").cloned().unwrap_or(Value::Null),
                "layer_table": gds.get("layer_table").cloned().unwrap_or(Value::Null),
                "frame": gds.get("tapeout_frame").cloned().unwrap_or(Value::Null),
                "dummy_fill_tiles": gds.get("dummy_fill_tiles").cloned().unwrap_or(Value::Null),
                "job_deck": gds.get("job_deck").cloned().unwrap_or(Value::Null),
            },
            "test_plan": test_plan,
        });

        let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
        write("manifest.json", &manifest_bytes)?;

        Ok(json!({
            "submission_dir": dir.to_string_lossy(),
            "files": ["chip.gds", "drc_report.json", "oqfp_spec.json", "manifest.json"],
            "manifest": manifest,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qorchestrate_core::stage::StageContext;
    use serde_json::json;

    fn test_ctx() -> (StageContext, uuid::Uuid) {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        let run_id = uuid::Uuid::now_v7();
        let ctx = StageContext::new(
            run_id,
            "tapeout_package",
            "http://localhost:8765",
            "http://localhost:8420",
            std::path::PathBuf::from("/tmp/test.brain"),
            tx,
        );
        (ctx, run_id)
    }

    #[test]
    fn decode_hex_roundtrips() {
        assert_eq!(decode_hex("00ff10"), Some(vec![0x00, 0xff, 0x10]));
        assert_eq!(decode_hex("abc"), None); // odd length
        assert_eq!(decode_hex("zz"), None); // non-hex
    }

    #[test]
    fn sha256_is_stable() {
        // Known SHA-256 of the empty input.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[tokio::test]
    async fn writes_full_submission_bundle() {
        let tmp = std::env::temp_dir().join(format!("tapeout-test-{}", uuid::Uuid::now_v7()));
        let (ctx, run_id) = test_ctx();

        // "deadbeef" -> 4 bytes of GDS payload.
        let input = json!({
            "output_dir": tmp.to_string_lossy(),
            "gds_generate_output": {
                "hex": "deadbeef",
                "num_qubits": 9,
                "num_resonators": 9,
                "num_bus_couplers": 12
            },
            "drc_check_output": { "clean": false, "num_violations": 3274, "deck": "coplanar_university" },
            "oqfp_build_output": { "oqfp_spec": { "oqfp_version": "1.0" } },
            "oqfp_validate_output": { "validated": true }
        });

        let out = TapeoutPackageStage::new()
            .execute_raw(input, &ctx)
            .await
            .expect("tapeout package should succeed");

        let dir = tmp.join(run_id.to_string());
        for f in ["chip.gds", "drc_report.json", "oqfp_spec.json", "manifest.json"] {
            assert!(dir.join(f).exists(), "{f} should be written");
        }

        // chip.gds bytes match the decoded hex.
        assert_eq!(std::fs::read(dir.join("chip.gds")).unwrap(), vec![0xde, 0xad, 0xbe, 0xef]);

        // Manifest carries the integrity hash + counts + validated flag.
        let man = &out["manifest"];
        assert_eq!(
            man["files"]["chip.gds"]["sha256"],
            json!("5f78c33274e43fa9de5659265c1d917e25c03722dcb0b8d27db8d5feaa813953")
        );
        assert_eq!(man["chip"]["num_qubits"], json!(9));
        assert_eq!(man["files"]["oqfp_spec.json"]["validated"], json!(true));
        assert_eq!(man["files"]["drc_report.json"]["num_violations"], json!(3274));
        assert_eq!(man["files"]["drc_report.json"]["deck"], json!("coplanar_university"));
        assert!(man["test_plan"].as_array().is_some_and(|a| !a.is_empty()));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn applies_foundry_profile() {
        let tmp = std::env::temp_dir().join(format!("tapeout-test-{}", uuid::Uuid::now_v7()));
        let (ctx, _) = test_ctx();
        let input = json!({
            "output_dir": tmp.to_string_lossy(),
            "foundry": "commercial_foundry",
            "gds_generate_output": { "hex": "deadbeef", "num_qubits": 4 },
            "drc_check_output": { "clean": true, "num_violations": 0, "deck": "coplanar_foundry" },
            "oqfp_build_output": { "oqfp_spec": {} },
            "oqfp_validate_output": { "validated": true }
        });
        let out = TapeoutPackageStage::new()
            .execute_raw(input, &ctx)
            .await
            .expect("tapeout ok");
        let man = &out["manifest"];
        // Profile drives process string, min feature, and test plan.
        assert_eq!(man["foundry"], json!("commercial_foundry"));
        assert_eq!(man["fab_process"], json!("AlOx_0.25um"));
        assert_eq!(man["min_feature_nm"], json!(250.0));
        let plan = man["test_plan"].as_array().expect("test_plan array");
        assert!(plan.iter().any(|s| s == "junction_resistance_map"),
            "commercial_foundry test plan should include foundry-specific steps");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[tokio::test]
    async fn errors_without_gds() {
        let (ctx, _) = test_ctx();
        let result = TapeoutPackageStage::new()
            .execute_raw(json!({ "oqfp_build_output": {} }), &ctx)
            .await;
        assert!(matches!(result, Err(StageError::InvalidInput(_))));
    }
}
