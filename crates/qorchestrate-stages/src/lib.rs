pub mod advanced;
pub mod bench;
pub mod chip;
pub mod explore;
pub mod fabrication;
pub mod meta;
pub mod oqfp;
pub mod physics;
pub mod process;
pub mod pulse;
pub mod qec;
pub mod qpudidp;
pub mod twin;

use std::path::PathBuf;
use std::sync::Arc;

use qorchestrate_core::stage::StageType;
use qorchestrate_executor::{PipelineExecutor, StageRegistry};

/// Register all standard (non-meta) stages into the provided registry.
///
/// Meta stages that require an `Arc<PipelineExecutor>` reference must be
/// registered separately via [`register_meta_stages`] after the executor has
/// been constructed.
pub fn register_standard_stages(registry: &mut StageRegistry) {
    registry.register(
        StageType::QpudidpRmflow,
        Arc::new(qpudidp::rmflow::RmflowStage::new()),
    );
    registry.register(
        StageType::QpudidpCmaes,
        Arc::new(qpudidp::cmaes::CmaesStage::new()),
    );
    registry.register(
        StageType::QemSolve,
        Arc::new(physics::qem_solve::QemSolveStage::new()),
    );
    registry.register(
        StageType::QemSweep,
        Arc::new(physics::qem_sweep::QemSweepStage::new()),
    );
    registry.register(
        StageType::BbqQuantize,
        Arc::new(physics::bbq_quantize::BbqQuantizeStage::new()),
    );
    registry.register(
        StageType::ScqSimulate,
        Arc::new(physics::scq_simulate::ScqSimulateStage::new()),
    );
    registry.register(
        StageType::FreqOptimize,
        Arc::new(chip::freq_optimize::FreqOptimizeStage::new()),
    );
    registry.register(
        StageType::XtalkAnalyze,
        Arc::new(chip::xtalk_analyze::XtalkAnalyzeStage::new()),
    );
    registry.register(
        StageType::ReadoutDesign,
        Arc::new(chip::readout_design::ReadoutDesignStage::new()),
    );
    registry.register(
        StageType::GrapeOptimize,
        Arc::new(pulse::grape_optimize::GrapeOptimizeStage::new()),
    );
    registry.register(
        StageType::DragOptimize,
        Arc::new(pulse::drag_optimize::DragOptimizeStage::new()),
    );
    registry.register(
        StageType::PqecAssess,
        Arc::new(qec::pqec_assess::PqecAssessStage::new()),
    );
    registry.register(
        StageType::QecThreshold,
        Arc::new(qec::qec_threshold::QecThresholdStage::new()),
    );
    registry.register(
        StageType::QecCompile,
        Arc::new(qec::qec_compile::QecCompileStage::new()),
    );
    registry.register(
        StageType::SurgeryResources,
        Arc::new(qec::surgery::SurgeryStage::new()),
    );
    registry.register(
        StageType::TwinCompare,
        Arc::new(twin::compare::TwinCompareStage::new()),
    );
    registry.register(
        StageType::TwinRecalibrate,
        Arc::new(twin::recalibrate::TwinRecalibrateStage::new()),
    );
    registry.register(
        StageType::TwinQecUpdate,
        Arc::new(twin::qec_update::TwinQecUpdateStage::new()),
    );
    registry.register(
        StageType::TwinMock,
        Arc::new(twin::mock::TwinMockStage::new()),
    );
    registry.register(
        StageType::MetrologyIngest,
        Arc::new(twin::metrology_ingest::MetrologyIngestStage::new()),
    );
    registry.register(
        StageType::MetrologyAcquire,
        Arc::new(twin::metrology_acquire::MetrologyAcquireStage::new()),
    );
    registry.register(
        StageType::RecalDispatch,
        Arc::new(twin::recal_dispatch::RecalDispatchStage::new()),
    );
    registry.register(
        StageType::QexplorePareto,
        Arc::new(explore::pareto::QexploreParetoStage::new()),
    );
    registry.register(
        StageType::FreqYield,
        Arc::new(explore::yield_report::YieldReportBuildStage::new()),
    );
    registry.register(
        StageType::BenchPredict,
        Arc::new(bench::predict::BenchPredictStage::new()),
    );
    registry.register(
        StageType::OqfpBuild,
        Arc::new(oqfp::build::OqfpBuildStage::new()),
    );
    registry.register(
        StageType::OqfpValidate,
        Arc::new(oqfp::validate::OqfpValidateStage::new()),
    );
    registry.register(
        StageType::GdsGenerate,
        Arc::new(fabrication::gds_generate::GdsGenerateStage::new()),
    );
    registry.register(
        StageType::DrcCheck,
        Arc::new(fabrication::drc_check::DrcCheckStage::new()),
    );
    registry.register(
        StageType::TapeoutPackage,
        Arc::new(fabrication::tapeout::TapeoutPackageStage::new()),
    );
    registry.register(
        StageType::ProcessRecipe,
        Arc::new(fabrication::process_recipe::ProcessRecipeStage::new()),
    );
    registry.register(
        StageType::Skip,
        Arc::new(meta::skip::SkipStage::new()),
    );
    registry.register(
        StageType::CollectDeps,
        Arc::new(meta::collect_deps::CollectDepsStage::new()),
    );
    registry.register(
        StageType::HttpPost,
        Arc::new(meta::http_post::HttpPostStage::new()),
    );
    // ── Parametric process design (rustyqcirc) ─────────────────────────────
    registry.register(
        StageType::QcircQuantize,
        Arc::new(process::quantize::QcircQuantizeStage::new()),
    );
    registry.register(
        StageType::QcircProcesses,
        Arc::new(process::processes::QcircProcessesStage::new()),
    );
    registry.register(
        StageType::QcircPumpDesign,
        Arc::new(process::pump_design::QcircPumpDesignStage::new()),
    );
    registry.register(
        StageType::QcircFloquet,
        Arc::new(process::floquet::QcircFloquetStage::new()),
    );
    registry.register(
        StageType::QcircRegimeScan,
        Arc::new(process::regime_scan::QcircRegimeScanStage::new()),
    );
    registry.register(
        StageType::QcircConstraints,
        Arc::new(process::constraints::QcircConstraintsStage::new()),
    );
    registry.register(
        StageType::QcircSummary,
        Arc::new(process::summary::QcircSummaryStage::new()),
    );
    // Non-SC modality stages (ion / atom / spin) were retired in favor of
    // the generic `http_post` meta stage — every modality template now
    // declares `type = "http_post", params.path = "/q<mod>/<op>"` instead
    // of needing a hand-written stage struct per endpoint. See the §4.7
    // follow-up⁴ note in quantum-consolidation-audit.md.
}

/// Register meta stages that need to dispatch sub-pipelines through the
/// executor.
///
/// The `executor` is a `Weak` ref because each meta stage gets stored inside
/// the registry, which the executor itself holds via `Arc<StageRegistry>` —
/// a strong `Arc<PipelineExecutor>` here would form a reference cycle and
/// leak. The expected construction pattern in callers is:
///
/// ```ignore
/// let executor: Arc<PipelineExecutor> = Arc::new_cyclic(|weak| {
///     let mut registry = StageRegistry::new();
///     register_standard_stages(&mut registry);
///     register_meta_stages(&mut registry, weak.clone(), templates_dir);
///     PipelineExecutor::new(Arc::new(registry), checkpoint, api_url, qpu_url)
/// });
/// ```
pub fn register_meta_stages(
    registry: &mut StageRegistry,
    executor: std::sync::Weak<PipelineExecutor>,
    templates_dir: PathBuf,
) {
    registry.register(
        StageType::PipelineCall,
        Arc::new(meta::pipeline_call::PipelineCallStage::new(
            executor.clone(),
            templates_dir.clone(),
        )),
    );
    registry.register(
        StageType::Batch,
        Arc::new(meta::batch::BatchStage::new(
            executor.clone(),
            templates_dir.clone(),
        )),
    );
    registry.register(
        StageType::BayesianOuterLoop,
        Arc::new(advanced::bayesian_outer_loop::BayesianOuterLoopStage::new(
            executor.clone(),
            templates_dir.clone(),
        )),
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use qorchestrate_core::{
        errors::StageError,
        stage::{Stage, StageContext, StageType},
    };
    use qorchestrate_executor::StageRegistry;
    use serde_json::{json, Value};

    fn test_ctx() -> StageContext {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        StageContext::new(
            uuid::Uuid::now_v7(),
            "test_stage",
            "http://localhost:8765",
            "http://localhost:8420",
            std::path::PathBuf::from("/tmp/test.brain"),
            tx,
        )
    }

    // ── 1. register_standard_stages populates expected stage types ───────────

    #[test]
    fn test_register_all_stages() {
        let mut registry = StageRegistry::new();
        register_standard_stages(&mut registry);

        assert!(registry.has(&StageType::FreqOptimize));
        assert!(registry.has(&StageType::XtalkAnalyze));
        assert!(registry.has(&StageType::OqfpBuild));
        assert!(registry.has(&StageType::OqfpValidate));
        assert!(registry.has(&StageType::GdsGenerate));
        assert!(registry.has(&StageType::DrcCheck));
        assert!(registry.has(&StageType::TapeoutPackage));
        assert!(registry.has(&StageType::ProcessRecipe));
        assert!(registry.has(&StageType::Skip));
        assert!(registry.has(&StageType::QpudidpRmflow));
        assert!(registry.has(&StageType::QpudidpCmaes));
        assert!(registry.has(&StageType::QemSolve));
        assert!(registry.has(&StageType::ScqSimulate));
        assert!(registry.has(&StageType::ReadoutDesign));
        assert!(registry.has(&StageType::GrapeOptimize));
        assert!(registry.has(&StageType::DragOptimize));
        assert!(registry.has(&StageType::PqecAssess));
        assert!(registry.has(&StageType::QecThreshold));
        assert!(registry.has(&StageType::QecCompile));
        assert!(registry.has(&StageType::SurgeryResources));
        assert!(registry.has(&StageType::TwinCompare));
        assert!(registry.has(&StageType::TwinRecalibrate));
        assert!(registry.has(&StageType::TwinQecUpdate));
        assert!(registry.has(&StageType::TwinMock));
        assert!(registry.has(&StageType::MetrologyIngest));
        assert!(registry.has(&StageType::MetrologyAcquire));
        assert!(registry.has(&StageType::QexplorePareto));
        assert!(registry.has(&StageType::FreqYield));
        assert!(registry.has(&StageType::BenchPredict));

        // Parametric process stages
        assert!(registry.has(&StageType::QcircQuantize));
        assert!(registry.has(&StageType::QcircProcesses));
        assert!(registry.has(&StageType::QcircPumpDesign));
        assert!(registry.has(&StageType::QcircFloquet));
        assert!(registry.has(&StageType::QcircRegimeScan));
        assert!(registry.has(&StageType::QcircConstraints));
        assert!(registry.has(&StageType::QcircSummary));

        // Meta stages not registered by register_standard_stages
        assert!(!registry.has(&StageType::PipelineCall));
        assert!(!registry.has(&StageType::Batch));
        assert!(!registry.has(&StageType::BayesianOuterLoop));
    }

    // ── 2. OqfpBuildStage assembles layers from upstream outputs ─────────────

    #[tokio::test]
    async fn test_oqfp_build_stage() {
        let stage = oqfp::build::OqfpBuildStage::new();
        let ctx = test_ctx();

        let input = json!({
            "freq_plan_output": {
                "assignments": [
                    {"qubit": 0, "frequency_ghz": 5.0},
                    {"qubit": 1, "frequency_ghz": 5.3}
                ],
                "collision_free": true,
                "yield_estimate": 0.82
            },
            "qec_assess_output": {
                "meets_threshold": true,
                "recommended_code": "surface_code",
                "max_achievable_distance": 5,
                "logical_error_rate": 1e-4
            }
        });

        let output = stage
            .execute_raw(input, &ctx)
            .await
            .expect("OqfpBuildStage should not fail for in-process logic");

        // Top-level keys
        assert_eq!(output.get("validated"), Some(&json!(false)));

        let spec = output.get("oqfp_spec").expect("oqfp_spec key present");
        let layers = spec.get("layers").expect("layers present");

        // Frequency layer
        let freq_layer = layers.get("frequency").expect("frequency layer");
        assert_eq!(
            freq_layer.get("collision_free"),
            Some(&json!(true)),
            "collision_free should propagate"
        );

        // QEC layer
        let qec_layer = layers.get("qec").expect("qec layer");
        assert_eq!(
            qec_layer.get("meets_threshold"),
            Some(&json!(true)),
            "meets_threshold should propagate"
        );
        assert_eq!(
            qec_layer.get("max_achievable_distance"),
            Some(&json!(5)),
        );

        // All eight layers present
        for layer_name in &[
            "device",
            "connectivity",
            "frequency",
            "qec",
            "control",
            "fabrication",
            "performance",
            "application",
        ] {
            assert!(
                layers.get(layer_name).is_some(),
                "layer '{}' must be present",
                layer_name
            );
        }
    }

    // ── 2a. OqfpBuildStage populates control / wiring / performance layers ────

    #[tokio::test]
    async fn test_oqfp_build_enriched_layers() {
        let stage = oqfp::build::OqfpBuildStage::new();
        let ctx = test_ctx();
        let input = json!({
            "freq_plan_output": { "assignments": [{"qubit": 0, "frequency_ghz": 5.1}], "yield_estimate": 0.8 },
            "scq_device_output": { "t1_us": [90.0, 110.0], "t2_us": [70.0, 80.0] },
            "pulse_optimize_output": { "pulse_shape": "DRAG", "duration_ns": 25.0, "fidelity": 0.9991 },
            "readout_design_output": { "readout_fidelity": 0.992 },
            "gds_generate_output": { "num_qubits": 8 },
        });
        let out = stage.execute_raw(input, &ctx).await.unwrap();
        let layers = out.get("oqfp_spec").unwrap().get("layers").unwrap();

        // Control: native gates + gate library + calibration targets.
        let control = layers.get("control").unwrap();
        assert_eq!(control.get("native_gates").unwrap().as_array().unwrap().len(), 5);
        let lib = control.get("gate_library").unwrap().as_array().unwrap();
        assert_eq!(lib.len(), 3);
        assert_eq!(lib[0].get("name").unwrap(), &json!("x"));
        assert_eq!(lib[0].get("fidelity").unwrap(), &json!(0.9991));
        assert!(control.get("calibration_targets").unwrap().as_array().unwrap()
            .iter().any(|c| c.get("parameter").unwrap() == &json!("t1_us")));

        // Fabrication wiring: a drive + readout + flux line per qubit.
        let wiring = layers.get("fabrication").unwrap().get("wiring").unwrap();
        assert_eq!(wiring.get("signal_lines").unwrap(), &json!(24)); // 8 qubits × 3
        assert_eq!(wiring.get("thermal_budget_ok").unwrap(), &json!(true));

        // Performance averages from scq / readout.
        let perf = layers.get("performance").unwrap();
        assert_eq!(perf.get("avg_t1_us").unwrap(), &json!(100.0)); // avg(90, 110)
        assert_eq!(perf.get("avg_t2_us").unwrap(), &json!(75.0)); // avg(70, 80)
        assert_eq!(perf.get("avg_readout_fidelity").unwrap(), &json!(0.992));

        // Application present.
        assert!(layers.get("application").unwrap().get("target_use_case").is_some());
    }

    // ── 2b. OqfpBuildStage populates fabrication layer from GDS + DRC ────────

    #[tokio::test]
    async fn test_oqfp_build_populates_fabrication_layer() {
        let stage = oqfp::build::OqfpBuildStage::new();
        let ctx = test_ctx();

        let input = json!({
            "freq_plan_output": { "yield_estimate": 0.77 },
            "gds_generate_output": {
                "lib_name": "quantum_api_chip",
                "n_bytes": 12345,
                "num_qubits": 9
            },
            "drc_check_output": { "clean": true, "num_violations": 0 }
        });

        let output = stage.execute_raw(input, &ctx).await.expect("build ok");
        let fab = output
            .get("oqfp_spec")
            .and_then(|s| s.get("layers"))
            .and_then(|l| l.get("fabrication"))
            .expect("fabrication layer present");

        assert_eq!(fab.get("gds_file"), Some(&json!("quantum_api_chip")));
        assert_eq!(fab.get("gds_n_bytes"), Some(&json!(12345)));
        assert_eq!(fab.get("num_qubits"), Some(&json!(9)));
        assert_eq!(fab.get("drc_clean"), Some(&json!(true)));
        assert_eq!(fab.get("drc_num_violations"), Some(&json!(0)));
        assert!(fab.get("process_params").is_some(), "process_params present");
    }

    // ── 2c. OqfpBuildStage populates device.junction from process_recipe ─────

    #[tokio::test]
    async fn test_oqfp_build_populates_junction() {
        let stage = oqfp::build::OqfpBuildStage::new();
        let ctx = test_ctx();
        let input = json!({
            "process_recipe_output": {
                "recipe": { "name": "dolan_alox_standard" },
                "eval": { "lj_nh": 12.8, "ic_ua": 0.0257, "area_um2": 0.03, "junction_sigma_percent": 4.55 }
            }
        });
        let output = stage.execute_raw(input, &ctx).await.expect("build ok");
        let layers = output.get("oqfp_spec").and_then(|s| s.get("layers")).unwrap();
        let junction = layers.get("device").and_then(|d| d.get("junction")).expect("device.junction");
        assert_eq!(junction.get("lj_nh"), Some(&json!(12.8)));
        assert_eq!(junction.get("area_um2"), Some(&json!(0.03)));
        let fab = layers.get("fabrication").unwrap();
        assert_eq!(fab.get("junction_recipe"), Some(&json!("dolan_alox_standard")));
        assert_eq!(fab.get("junction_sigma_percent"), Some(&json!(4.55)));
    }

    // ── 3. OqfpValidateStage accepts a well-formed spec ─────────────────────

    #[tokio::test]
    async fn test_oqfp_validate_stage_valid() {
        let stage = oqfp::validate::OqfpValidateStage::new();
        let ctx = test_ctx();

        let spec = json!({
            "oqfp_version": "1.0",
            "layers": {
                "device": {},
                "connectivity": {},
                "frequency": {},
                "qec": {},
                "control": {},
                "fabrication": {},
                "performance": {},
                "application": {}
            }
        });

        let input = json!({ "oqfp_spec": spec });

        let output = stage
            .execute_raw(input, &ctx)
            .await
            .expect("valid spec should not fail validation");

        assert_eq!(output.get("validated"), Some(&json!(true)));
        assert_eq!(
            output.get("errors"),
            Some(&json!([])),
            "errors array should be empty"
        );
    }

    // ── 4. OqfpValidateStage rejects a spec missing the qec layer ───────────

    #[tokio::test]
    async fn test_oqfp_validate_stage_invalid() {
        let stage = oqfp::validate::OqfpValidateStage::new();
        let ctx = test_ctx();

        // Deliberately omit the "qec" layer
        let spec = json!({
            "oqfp_version": "1.0",
            "layers": {
                "device": {},
                "connectivity": {},
                "frequency": {},
                // "qec" intentionally missing
                "control": {},
                "fabrication": {},
                "performance": {},
                "application": {}
            }
        });

        let input = json!({ "oqfp_spec": spec });

        let result = stage.execute_raw(input, &ctx).await;

        match result {
            Err(StageError::InvalidInput(msg)) => {
                assert!(
                    msg.contains("qec"),
                    "error message should name the missing layer; got: {}",
                    msg
                );
            }
            other => panic!(
                "expected StageError::InvalidInput for missing qec layer, got: {:?}",
                other
            ),
        }
    }

    // ── 5. SkipStage returns {{skipped: true}} ───────────────────────────────

    #[tokio::test]
    async fn test_skip_stage() {
        let stage = meta::skip::SkipStage::new();
        let ctx = test_ctx();

        let output = stage
            .execute_raw(json!({}), &ctx)
            .await
            .expect("SkipStage must always succeed");

        assert_eq!(
            output.get("skipped"),
            Some(&Value::Bool(true)),
            "output must contain skipped: true"
        );
    }

    // ── 6. OqfpBuildStage handles fully absent upstream outputs gracefully ───

    #[tokio::test]
    async fn test_oqfp_build_stage_empty_input() {
        let stage = oqfp::build::OqfpBuildStage::new();
        let ctx = test_ctx();

        let output = stage
            .execute_raw(json!({}), &ctx)
            .await
            .expect("OqfpBuildStage must not fail on empty input");

        let spec = output.get("oqfp_spec").expect("oqfp_spec present");
        let layers = spec.get("layers").expect("layers present");

        // Defaults: collision_free should be false when no upstream data
        let freq = layers.get("frequency").expect("frequency layer present");
        assert_eq!(freq.get("collision_free"), Some(&json!(false)));
    }

    // ── 7. OqfpValidateStage accepts spec nested under oqfp_build_output ────

    #[tokio::test]
    async fn test_oqfp_validate_accepts_build_output_wrapper() {
        let stage = oqfp::validate::OqfpValidateStage::new();
        let ctx = test_ctx();

        let spec = json!({
            "oqfp_version": "1.0",
            "layers": {
                "device": {},
                "connectivity": {},
                "frequency": {},
                "qec": {},
                "control": {},
                "fabrication": {},
                "performance": {},
                "application": {}
            }
        });

        // Simulate what OqfpBuildStage actually returns
        let input = json!({
            "oqfp_build_output": {
                "oqfp_spec": spec,
                "validated": false
            }
        });

        let output = stage
            .execute_raw(input, &ctx)
            .await
            .expect("should accept oqfp_build_output wrapper");

        assert_eq!(output.get("validated"), Some(&json!(true)));
    }

    // ── 8. QcircSummaryStage assembles report from upstream outputs ──────────

    #[tokio::test]
    async fn test_qcirc_summary_stage() {
        let stage = process::summary::QcircSummaryStage::new();
        let ctx = test_ctx();

        let input = json!({
            "qcirc_processes_output": {
                "processes": [
                    { "process_type": "ThreeWaveMixing", "coupling_mhz": 50.0 },
                    { "process_type": "Kerr", "coupling_mhz": 10.0 }
                ]
            },
            "qcirc_pump_design_output": {
                "pump_type": "FluxModulation",
                "pump_frequency_ghz": 11.5,
                "pump_amplitude": 0.05
            },
            "qcirc_floquet_output": {
                "n_collisions": 0,
                "collision_free": true,
                "quasi_energies": []
            },
            "qcirc_regime_scan_output": {
                "pareto_front": [
                    { "effective_coupling_mhz": 48.0, "heating_rate_mhz": 0.1 },
                    { "effective_coupling_mhz": 45.0, "heating_rate_mhz": 0.05 }
                ]
            },
            "qcirc_constraints_output": {
                "ej_range_ghz": [18.0, 25.0],
                "alpha_range": [0.15, 0.25]
            }
        });

        let output = stage.execute_raw(input, &ctx).await.expect("summary must not fail");

        let summary = output.get("summary").expect("summary key present");
        assert_eq!(summary.get("dominant_process"), Some(&json!("ThreeWaveMixing")));
        assert_eq!(summary.get("n_processes_identified"), Some(&json!(2)));
        assert_eq!(summary.get("pump_type"), Some(&json!("FluxModulation")));
        assert_eq!(summary.get("floquet_collision_free"), Some(&json!(true)));
        assert_eq!(summary.get("n_pareto_regime_points"), Some(&json!(2)));

        // All top-level sections present
        assert!(output.get("processes").is_some());
        assert!(output.get("pump_design").is_some());
        assert!(output.get("floquet").is_some());
        assert!(output.get("regime_scan").is_some());
        assert!(output.get("circuit_constraints").is_some());
    }

    // ── 9. QcircSummaryStage handles missing upstream outputs gracefully ─────

    #[tokio::test]
    async fn test_qcirc_summary_stage_empty_input() {
        let stage = process::summary::QcircSummaryStage::new();
        let ctx = test_ctx();

        let output = stage
            .execute_raw(json!({}), &ctx)
            .await
            .expect("summary must not fail on empty input");

        let summary = output.get("summary").expect("summary key present");
        assert_eq!(summary.get("dominant_process"), Some(&json!("unknown")));
        assert_eq!(summary.get("n_processes_identified"), Some(&json!(0)));
        assert_eq!(summary.get("n_pareto_regime_points"), Some(&json!(0)));
    }

    // ── 10. stage_type() and timeout_secs() return correct values ────────────

    #[test]
    fn test_stage_metadata() {
        use qorchestrate_core::stage::Stage;

        let skip = meta::skip::SkipStage::new();
        assert_eq!(skip.stage_type(), StageType::Skip);
        assert_eq!(skip.timeout_secs(), 1);

        let oqfp_build = oqfp::build::OqfpBuildStage::new();
        assert_eq!(oqfp_build.stage_type(), StageType::OqfpBuild);
        assert_eq!(oqfp_build.timeout_secs(), 10);

        let oqfp_validate = oqfp::validate::OqfpValidateStage::new();
        assert_eq!(oqfp_validate.stage_type(), StageType::OqfpValidate);
        assert_eq!(oqfp_validate.timeout_secs(), 5);

        let freq = chip::freq_optimize::FreqOptimizeStage::new();
        assert_eq!(freq.stage_type(), StageType::FreqOptimize);
        assert_eq!(freq.timeout_secs(), 30);

        let grape = pulse::grape_optimize::GrapeOptimizeStage::new();
        assert_eq!(grape.stage_type(), StageType::GrapeOptimize);
        assert_eq!(grape.timeout_secs(), 120);

        let qcirc_quantize = process::quantize::QcircQuantizeStage::new();
        assert_eq!(qcirc_quantize.stage_type(), StageType::QcircQuantize);
        assert_eq!(qcirc_quantize.timeout_secs(), 30);

        let qcirc_summary = process::summary::QcircSummaryStage::new();
        assert_eq!(qcirc_summary.stage_type(), StageType::QcircSummary);
        assert_eq!(qcirc_summary.timeout_secs(), 5);
    }

    // ── 11. All HTTP stages: stage_type() + new() ────────────────────────────
    //
    // Each stage's new() and stage_type() are exercised here.
    // execute_raw() requires a live quantum-api — covered by integration tests.

    #[test]
    fn test_all_stage_types_constructible() {
        use qorchestrate_core::stage::Stage;

        // qpudidp
        let s = qpudidp::rmflow::RmflowStage::new();
        assert_eq!(s.stage_type(), StageType::QpudidpRmflow);
        let s = qpudidp::cmaes::CmaesStage::new();
        assert_eq!(s.stage_type(), StageType::QpudidpCmaes);

        // physics
        let s = physics::qem_solve::QemSolveStage::new();
        assert_eq!(s.stage_type(), StageType::QemSolve);
        let s = physics::scq_simulate::ScqSimulateStage::new();
        assert_eq!(s.stage_type(), StageType::ScqSimulate);

        // chip
        let s = chip::xtalk_analyze::XtalkAnalyzeStage::new();
        assert_eq!(s.stage_type(), StageType::XtalkAnalyze);
        let s = chip::readout_design::ReadoutDesignStage::new();
        assert_eq!(s.stage_type(), StageType::ReadoutDesign);

        // pulse
        let s = pulse::drag_optimize::DragOptimizeStage::new();
        assert_eq!(s.stage_type(), StageType::DragOptimize);

        // qec
        let s = qec::pqec_assess::PqecAssessStage::new();
        assert_eq!(s.stage_type(), StageType::PqecAssess);
        let s = qec::qec_threshold::QecThresholdStage::new();
        assert_eq!(s.stage_type(), StageType::QecThreshold);
        let s = qec::qec_compile::QecCompileStage::new();
        assert_eq!(s.stage_type(), StageType::QecCompile);
        let s = qec::surgery::SurgeryStage::new();
        assert_eq!(s.stage_type(), StageType::SurgeryResources);

        // explore
        let s = explore::pareto::QexploreParetoStage::new();
        assert_eq!(s.stage_type(), StageType::QexplorePareto);
        let s = explore::yield_report::YieldReportBuildStage::new();
        assert_eq!(s.stage_type(), StageType::FreqYield);

        // bench
        let s = bench::predict::BenchPredictStage::new();
        assert_eq!(s.stage_type(), StageType::BenchPredict);

        // twin
        let s = twin::compare::TwinCompareStage::new();
        assert_eq!(s.stage_type(), StageType::TwinCompare);
        let s = twin::recalibrate::TwinRecalibrateStage::new();
        assert_eq!(s.stage_type(), StageType::TwinRecalibrate);
        let s = twin::qec_update::TwinQecUpdateStage::new();
        assert_eq!(s.stage_type(), StageType::TwinQecUpdate);
        let s = twin::mock::TwinMockStage::new();
        assert_eq!(s.stage_type(), StageType::TwinMock);
        let s = twin::metrology_ingest::MetrologyIngestStage::new();
        assert_eq!(s.stage_type(), StageType::MetrologyIngest);
        let s = twin::metrology_acquire::MetrologyAcquireStage::new();
        assert_eq!(s.stage_type(), StageType::MetrologyAcquire);

        // fabrication
        let s = fabrication::gds_generate::GdsGenerateStage::new();
        assert_eq!(s.stage_type(), StageType::GdsGenerate);
        let s = fabrication::drc_check::DrcCheckStage::new();
        assert_eq!(s.stage_type(), StageType::DrcCheck);
        let s = fabrication::tapeout::TapeoutPackageStage::new();
        assert_eq!(s.stage_type(), StageType::TapeoutPackage);
        let s = fabrication::process_recipe::ProcessRecipeStage::new();
        assert_eq!(s.stage_type(), StageType::ProcessRecipe);

        // process
        let s = process::processes::QcircProcessesStage::new();
        assert_eq!(s.stage_type(), StageType::QcircProcesses);
        let s = process::pump_design::QcircPumpDesignStage::new();
        assert_eq!(s.stage_type(), StageType::QcircPumpDesign);
        let s = process::floquet::QcircFloquetStage::new();
        assert_eq!(s.stage_type(), StageType::QcircFloquet);
        let s = process::regime_scan::QcircRegimeScanStage::new();
        assert_eq!(s.stage_type(), StageType::QcircRegimeScan);
        let s = process::constraints::QcircConstraintsStage::new();
        assert_eq!(s.stage_type(), StageType::QcircConstraints);
    }

    #[test]
    fn test_all_stage_timeouts_are_positive() {
        use qorchestrate_core::stage::Stage;
        let stages: Vec<(&str, Box<dyn Stage>)> = vec![
            ("rmflow",        Box::new(qpudidp::rmflow::RmflowStage::new())),
            ("cmaes",         Box::new(qpudidp::cmaes::CmaesStage::new())),
            ("qem_solve",     Box::new(physics::qem_solve::QemSolveStage::new())),
            ("scq_simulate",  Box::new(physics::scq_simulate::ScqSimulateStage::new())),
            ("freq_opt",      Box::new(chip::freq_optimize::FreqOptimizeStage::new())),
            ("xtalk",         Box::new(chip::xtalk_analyze::XtalkAnalyzeStage::new())),
            ("readout",       Box::new(chip::readout_design::ReadoutDesignStage::new())),
            ("grape",         Box::new(pulse::grape_optimize::GrapeOptimizeStage::new())),
            ("drag",          Box::new(pulse::drag_optimize::DragOptimizeStage::new())),
            ("pqec",          Box::new(qec::pqec_assess::PqecAssessStage::new())),
            ("qec_thresh",    Box::new(qec::qec_threshold::QecThresholdStage::new())),
            ("surgery",       Box::new(qec::surgery::SurgeryStage::new())),
            ("pareto",        Box::new(explore::pareto::QexploreParetoStage::new())),
            ("yield",         Box::new(explore::yield_report::YieldReportBuildStage::new())),
            ("bench",         Box::new(bench::predict::BenchPredictStage::new())),
            ("twin_cmp",      Box::new(twin::compare::TwinCompareStage::new())),
            ("twin_recal",    Box::new(twin::recalibrate::TwinRecalibrateStage::new())),
            ("twin_qec",      Box::new(twin::qec_update::TwinQecUpdateStage::new())),
            ("twin_mock",     Box::new(twin::mock::TwinMockStage::new())),
            ("oqfp_build",    Box::new(oqfp::build::OqfpBuildStage::new())),
            ("oqfp_valid",    Box::new(oqfp::validate::OqfpValidateStage::new())),
            ("skip",          Box::new(meta::skip::SkipStage::new())),
            ("qcirc_quant",   Box::new(process::quantize::QcircQuantizeStage::new())),
            ("qcirc_proc",    Box::new(process::processes::QcircProcessesStage::new())),
            ("qcirc_pump",    Box::new(process::pump_design::QcircPumpDesignStage::new())),
            ("qcirc_floq",    Box::new(process::floquet::QcircFloquetStage::new())),
            ("qcirc_regime",  Box::new(process::regime_scan::QcircRegimeScanStage::new())),
            ("qcirc_constr",  Box::new(process::constraints::QcircConstraintsStage::new())),
            ("qcirc_summ",    Box::new(process::summary::QcircSummaryStage::new())),
        ];
        for (name, stage) in &stages {
            assert!(
                stage.timeout_secs() > 0,
                "Stage '{}' should have positive timeout", name
            );
        }
    }

    // ── 12–16. Integration tests: execute_raw() against live quantum-api ──────
    //
    // These tests call real HTTP endpoints. They are skipped automatically when
    // the QUANTUM_API_URL environment variable is not set (or the API is down).
    // Run with:  QUANTUM_API_URL=http://localhost:8770 cargo test integration

    fn integration_api_url() -> Option<String> {
        std::env::var("QUANTUM_API_URL").ok()
    }

    fn integration_ctx(api_url: &str) -> StageContext {
        let (tx, _) = tokio::sync::broadcast::channel(16);
        StageContext::new(
            uuid::Uuid::now_v7(),
            "integration_test",
            api_url,
            "http://localhost:8420",
            std::path::PathBuf::from("/tmp/test.brain"),
            tx,
        )
    }

    #[tokio::test]
    async fn integration_bench_predict_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return, // skip if not configured
        };
        let stage = bench::predict::BenchPredictStage::new();
        let ctx = integration_ctx(&api_url);
        let input = serde_json::json!({
            "n_qubits": 20,
            "t1": 80,
            "t2": 60,
            "gate_fidelity": 0.999,
            "readout_fidelity": 0.997
        });
        let output = stage.execute_raw(input, &ctx).await
            .expect("bench/predict should succeed");
        assert!(output.get("chip_spec").is_some() || output.get("quantum_volume").is_some(),
            "bench/predict should return chip_spec or quantum_volume, got: {}", output);
    }

    #[tokio::test]
    async fn integration_freq_optimize_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = chip::freq_optimize::FreqOptimizeStage::new();
        let ctx = integration_ctx(&api_url);
        let input = serde_json::json!({
            "topology": "heavy_hex",
            "rows": 2,
            "cols": 2
        });
        let output = stage.execute_raw(input, &ctx).await
            .expect("freq/optimize should succeed");
        // Result is an array of qubit assignments or a JSON object
        assert!(
            output.is_array() || output.get("assignments").is_some() || output.get("collision_free").is_some(),
            "freq/optimize should return assignments, got: {}", output
        );
    }

    #[tokio::test]
    async fn integration_readout_design_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = chip::readout_design::ReadoutDesignStage::new();
        let ctx = integration_ctx(&api_url);
        let input = serde_json::json!({
            "qubit_freq": 5.0,
            "anharmonicity": -250.0,
            "target_fidelity": 0.999
        });
        let output = stage.execute_raw(input, &ctx).await
            .expect("readout/design should succeed");
        assert!(
            output.get("resonator_freq_ghz").is_some() || output.get("purcell_filter").is_some()
                || output.get("anharmonicity_mhz").is_some(),
            "readout/design should return resonator params, got: {}", output
        );
    }

    #[tokio::test]
    async fn integration_scq_snail_via_quantize_stage() {
        // QcircQuantizeStage calls /qcirc/quantize; we can test scq_snail directly
        // by using the ScqSimulateStage → /scq/spectrum endpoint instead.
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = physics::scq_simulate::ScqSimulateStage::new();
        let ctx = integration_ctx(&api_url);
        let input = serde_json::json!({
            "circuit_type": "transmon",
            "ec": 0.3,
            "ej": 15.0,
            "n_evals": 5
        });
        let result = stage.execute_raw(input, &ctx).await;
        // Either succeeds (with spectrum) or returns a BackendError (misconfigured CLI)
        // In both cases the HTTP layer reached the server — that's what we test here.
        match result {
            Ok(output) => {
                assert!(
                    output.get("eigenvalues").is_some() || output.get("spectrum").is_some()
                        || output.get("f01_ghz").is_some() || output.get("error").is_some(),
                    "scq simulate: unexpected response shape: {}", output
                );
            }
            Err(e) => {
                // BackendError means the HTTP call succeeded but the CLI had an issue.
                // HttpError would mean the server is unreachable (shouldn't happen here).
                assert!(
                    !matches!(e, qorchestrate_core::errors::StageError::HttpError(_)),
                    "HTTP layer should be reachable: {:?}", e
                );
            }
        }
    }

    #[tokio::test]
    async fn integration_gds_generate_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = fabrication::gds_generate::GdsGenerateStage::new();
        let ctx = integration_ctx(&api_url);
        let input = serde_json::json!({
            "freq_plan_output": {
                "assignments": [{"qubit": 0}, {"qubit": 1}, {"qubit": 2}, {"qubit": 3}]
            }
        });
        let output = stage
            .execute_raw(input, &ctx)
            .await
            .expect("gds_generate should succeed against live API");
        assert_eq!(output.get("format"), Some(&json!("gds2")));
        assert!(
            output.get("n_bytes").and_then(|v| v.as_u64()).unwrap_or(0) > 0,
            "GDS output should be non-empty: {output}"
        );
        // chip_params must be echoed for drc_check to reuse.
        assert!(
            output.get("chip_params").and_then(|p| p.get("cols")).is_some(),
            "chip_params.cols should be echoed: {output}"
        );
    }

    #[tokio::test]
    async fn integration_drc_check_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        // First generate, then check the same layout (report-only).
        let gen_stage = fabrication::gds_generate::GdsGenerateStage::new();
        let drc = fabrication::drc_check::DrcCheckStage::new();
        let ctx = integration_ctx(&api_url);
        let gen_out = gen_stage
            .execute_raw(
                serde_json::json!({"freq_plan_output": {"assignments": [{"qubit": 0}, {"qubit": 1}]}}),
                &ctx,
            )
            .await
            .expect("gds_generate ok");
        let drc_input = serde_json::json!({ "gds_generate_output": gen_out });
        let report = drc
            .execute_raw(drc_input, &ctx)
            .await
            .expect("drc_check (report-only) should not fail");
        assert!(report.get("num_violations").is_some(), "report has num_violations: {report}");
        assert!(report.get("clean").is_some(), "report has clean flag: {report}");
    }

    #[tokio::test]
    async fn integration_process_recipe_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = fabrication::process_recipe::ProcessRecipeStage::new();
        let ctx = integration_ctx(&api_url);
        // foundry profile supplies the recipe name
        let out = stage
            .execute_raw(json!({ "foundry": "commercial_foundry" }), &ctx)
            .await
            .expect("process_recipe ok");
        let ej = out
            .get("eval")
            .and_then(|e| e.get("ej_ghz"))
            .and_then(|v| v.as_f64())
            .expect("eval.ej_ghz");
        assert!(ej > 5.0 && ej < 40.0, "E_J should be in transmon band, got {ej}");
        assert!(out.get("process_params").is_some(), "process_params present");
    }

    #[tokio::test]
    async fn integration_tapeout_package_execute_raw() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let ctx = integration_ctx(&api_url);

        // Real GDS from the live API, then a real DRC report over it.
        let gen_out = fabrication::gds_generate::GdsGenerateStage::new()
            .execute_raw(
                json!({"freq_plan_output": {"assignments": [{"qubit": 0}, {"qubit": 1}, {"qubit": 2}]}}),
                &ctx,
            )
            .await
            .expect("gds_generate ok");
        let drc_out = fabrication::drc_check::DrcCheckStage::new()
            .execute_raw(json!({ "gds_generate_output": gen_out.clone() }), &ctx)
            .await
            .expect("drc_check ok");

        let out = fabrication::tapeout::TapeoutPackageStage::new()
            .execute_raw(
                json!({
                    "gds_generate_output": gen_out,
                    "drc_check_output": drc_out,
                    "oqfp_build_output": { "oqfp_spec": { "oqfp_version": "1.0" } },
                    "oqfp_validate_output": { "validated": true }
                }),
                &ctx,
            )
            .await
            .expect("tapeout_package ok");

        let dir = out["submission_dir"].as_str().expect("submission_dir");
        assert!(std::path::Path::new(dir).join("chip.gds").exists());
        assert!(std::path::Path::new(dir).join("manifest.json").exists());
        // GDS-II files begin with a HEADER record (0x00 0x06 0x00 0x02 ...).
        let gds = std::fs::read(std::path::Path::new(dir).join("chip.gds")).unwrap();
        assert!(gds.len() > 4 && gds[0] == 0x00 && gds[2] == 0x00 && gds[3] == 0x02);
        assert!(out["manifest"]["files"]["chip.gds"]["sha256"].is_string());
    }

    #[tokio::test]
    async fn integration_bench_predict_returns_valid_structure() {
        let api_url = match integration_api_url() {
            Some(u) => u,
            None => return,
        };
        let stage = bench::predict::BenchPredictStage::new();
        let ctx = integration_ctx(&api_url);

        // Test with a large qubit count to exercise the CLI
        let input = serde_json::json!({
            "n_qubits": 50,
            "t1": 100,
            "t2": 80,
            "gate_fidelity": 0.9995,
            "readout_fidelity": 0.999
        });
        let output = stage.execute_raw(input, &ctx).await
            .expect("bench/predict with 50 qubits should succeed");

        // quantum_volume and CLOPS should be present in the chip_spec sub-object
        let chip = output.get("chip_spec").or_else(|| Some(&output)).unwrap();
        assert!(
            chip.get("quantum_volume").is_some()
                || chip.get("clops").is_some()
                || output.get("predictions").is_some()
                || output.get("chip_spec").is_some(),
            "bench/predict response should contain QV or CLOPS: {}", output
        );
    }
}
