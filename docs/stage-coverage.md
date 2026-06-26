# Orchestration stage coverage

How every quantum-api capability is reachable from an orchestration pipeline.

## Two ways a capability becomes a pipeline stage

1. **`http_post` (universal).** Any of the ~220 quantum-api POST endpoints is
   already usable as a pipeline stage with no Rust code:

   ```toml
   [[stage]]
   id = "vqe"
   type = "http_post"
   depends_on = ["molecule"]
   params = { path = "/qchem/vqe", ansatz = "uccsd" }
   ```

   The stage POSTs its accumulated input (plus `params`) to `path` and returns
   the JSON. This is the fallback for any endpoint without a dedicated name.

2. **Named capability stages (`ToolStage` table).** The meaningful capability
   groups also get a first-class, self-documenting `StageType` so pipelines can
   reference them by name (`type = "bench_qv"`) and the registry self-documents
   what is wired. These are registered table-driven from
   `qorchestrate-stages/src/generic.rs::TOOL_STAGES` — add a
   `(StageType, "/path")` row to name a new one.

   Named groups today: characterization (`bench_qv`, `bench_rb`, `cal_rb`,
   `cal_spectroscopy`, `cal_leakage_rb`), meshing (`mesh_transmon_cross`,
   `mesh_chip`, `mesh_quality`), wiring/packaging (`wiring_design`,
   `wiring_noise`, `pkg_design`, `pkg_wirebonds`, `cryo_analyze`, `cryo_power`),
   bosonic/codesign/surgery/stim (`bosonic_simulate`, `bosonic_optimize`,
   `codesign_optimize`, `codesign_roadmap`, `surgery_compile`, `stim_gen`),
   physics (`floquet_spectrum`, `floquet_propagator`, `scq_coherence`,
   `scq_spectrum`, `readout_fidelity`, `readout_multiplex`, `pulse_simulate`),
   applications (`qchem_molecule`, `qchem_vqe`, `qaoa_maxcut`, `qaoa_portfolio`,
   `qml_classify`, `qml_kernel`, `qnet_entangle`, `qnet_scale`), and
   transpile/symbolic/viz (`transpile_compile`, `symclaw_simplify`,
   `symclaw_solve`, `clawview_streamlines`), plus the orphaned-capability fixes
   (`extract_cpw`, `extract_tls`, `clawprint_dressed`, `fw_compile`).

## Dedicated stages with input mapping

A handful of stages do more than pass-through — they map upstream outputs into
the endpoint's request shape. These are hand-written (not in the table):
`qem_sweep` (inverse-design geometry → `FrequencySweepRequest`), `bbq_quantize`
(sweep `s_parameters` → `/bbq/quantize`), `pqec_assess` (pulse fidelity + device
coherence → QEC assessment), `recal_dispatch` (twin `TriggerInverseDesign` hints
→ targeted re-design), and the core design-to-chip stages.

## Example pipelines

- `characterization-suite.toml` — `bench_qv` + `bench_rb` + `cal_rb`.
- `em-to-hamiltonian.toml` — geometry → `qem_sweep` → `bbq_quantize` → qcirc.
- `atom/ion/spin-design.toml` — design + full physics budget (coherence/readout/
  crosstalk/frequency/pulse).
