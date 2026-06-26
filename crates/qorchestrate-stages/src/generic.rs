//! A generic, registry-driven stage that POSTs its input to a fixed quantum-api
//! endpoint and returns the JSON response. This lets any capability endpoint be a
//! first-class, *named* pipeline stage without a bespoke impl — register
//! `(StageType, "/path")` pairs in a table (see `register_tool_stages`).
//!
//! (The pre-existing `HttpPostStage` already gives universal endpoint reach via a
//! `params.path`; `ToolStage` adds a named StageType so pipelines reference the
//! capability by name and the registry self-documents what's wired.)

use async_trait::async_trait;
use reqwest::Client;
use serde_json::Value;

use qorchestrate_core::{
    errors::StageError,
    stage::{Stage, StageContext, StageType},
};

pub struct ToolStage {
    stage_type: StageType,
    path: &'static str,
    timeout: u64,
    client: Client,
}

impl ToolStage {
    pub fn new(stage_type: StageType, path: &'static str) -> Self {
        Self {
            stage_type,
            path,
            timeout: 120,
            client: Client::new(),
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.timeout = secs;
        self
    }
}

#[async_trait]
impl Stage for ToolStage {
    fn stage_type(&self) -> StageType {
        self.stage_type.clone()
    }

    fn timeout_secs(&self) -> u64 {
        self.timeout
    }

    async fn execute_raw(&self, input: Value, ctx: &StageContext) -> Result<Value, StageError> {
        let resp = self
            .client
            .post(format!("{}{}", ctx.quantum_api_url, self.path))
            .json(&input)
            .send()
            .await
            .map_err(|e| StageError::HttpError(e.to_string()))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let b = resp.text().await.unwrap_or_default();
            return Err(StageError::BackendError(format!("{}: {}", status, b)));
        }
        resp.json::<Value>()
            .await
            .map_err(|e| StageError::ParseError(e.to_string()))
    }
}

/// The table of capability endpoints exposed as named pipeline stages.
/// Add a `(StageType, endpoint_path)` row to make a capability first-class.
pub const TOOL_STAGES: &[(StageType, &str)] = &[
    // Parasitics / control / firmware (the previously orphaned capabilities)
    (StageType::ExtractCpw, "/extract/cpw"),
    (StageType::ExtractTls, "/extract/tls"),
    (StageType::ClawprintDressed, "/clawprint/dressed"),
    (StageType::FwCompile, "/qfw/compile"),
    // Characterization
    (StageType::BenchQv, "/bench/qv"),
    (StageType::BenchRb, "/bench/rb"),
    (StageType::CalRb, "/cal/rb"),
    (StageType::CalSpectroscopy, "/cal/spectroscopy"),
    (StageType::CalLeakageRb, "/cal/leakage-rb"),
    // Meshing
    (StageType::MeshTransmonCross, "/mesh/transmon-cross"),
    (StageType::MeshChip, "/mesh/chip"),
    (StageType::MeshQuality, "/mesh/quality"),
    // Cryo wiring / packaging
    (StageType::WiringDesign, "/wiring/design"),
    (StageType::WiringNoise, "/wiring/noise"),
    (StageType::PkgDesign, "/pkg/design"),
    (StageType::PkgWirebonds, "/pkg/wirebonds"),
    (StageType::CryoAnalyze, "/cryo/analyze"),
    (StageType::CryoPower, "/cryo/power"),
    // Bosonic / codesign / surgery / stim
    (StageType::BosonicSimulate, "/bosonic/simulate"),
    (StageType::BosonicOptimize, "/bosonic/optimize"),
    (StageType::CodesignOptimize, "/codesign/optimize"),
    (StageType::CodesignRoadmap, "/codesign/roadmap"),
    (StageType::SurgeryCompile, "/surgery/compile"),
    (StageType::StimGen, "/stim/gen"),
    // Floquet / scq / readout / pulse
    (StageType::FloquetSpectrum, "/floquet/spectrum"),
    (StageType::FloquetPropagator, "/floquet/propagator"),
    (StageType::ScqCoherence, "/scq/coherence"),
    (StageType::ScqSpectrum, "/scq/spectrum"),
    (StageType::ReadoutFidelity, "/readout/fidelity"),
    (StageType::ReadoutMultiplex, "/readout/multiplex"),
    (StageType::PulseSimulate, "/pulse/simulate"),
    // Applications
    (StageType::QchemMolecule, "/qchem/molecule"),
    (StageType::QchemVqe, "/qchem/vqe"),
    (StageType::QaoaMaxcut, "/qaoa/maxcut"),
    (StageType::QaoaPortfolio, "/qaoa/portfolio"),
    (StageType::QmlClassify, "/qml/classify"),
    (StageType::QmlKernel, "/qml/kernel"),
    (StageType::QnetEntangle, "/qnet/entangle"),
    (StageType::QnetScale, "/qnet/scale"),
    // Transpile / symbolic / viz
    (StageType::TranspileCompile, "/transpile/compile"),
    (StageType::SymclawSimplify, "/symclaw/simplify"),
    (StageType::SymclawSolve, "/symclaw/solve"),
    (StageType::ClawviewStreamlines, "/clawview/streamlines"),
];
