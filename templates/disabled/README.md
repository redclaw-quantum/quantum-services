# Disabled templates

Both templates here are functionally broken against the current
`quantum-api`. They live in this subdirectory so they don't show up in
`qorchestrate list-templates` (which only scans the top-level templates
directory) but are preserved with their original design intent so a
future enable-pass has a clean starting point.

## `parametric-amp-design.toml` — ~~fails at TOML parse time~~ **FIXED 2026-06-02**

Restored to `templates/` and runs end-to-end. The fix landed:

1. Replaced every CamelCase stage `type` with the canonical
   `http_post` against the right quantum-api endpoint. No
   `QemSolve`/`BbqQuantize`/`BbqToQcirc` stage variants needed.
2. Added `/bbq/to-qcirc` endpoint to `quantum-api` (wraps the existing
   `rustybbq to-qcirc` CLI subcommand). Auto-finds upstream BbqQuantResult
   via the `mode_frequencies_ghz` marker.
3. Patched `bbq-cli`'s `QuantizeOutput` to include `charging_energies_ghz`
   — pre-existing pipe-breakage where `rustybbq quantize` output didn't
   match the `BbqQuantResult` shape `rustybbq to-qcirc` required.
4. Added the `pump_specs → PumpConfiguration` translator in
   `/qcirc/floquet` so the bare PumpSpec[] that `qcirc pump-design` emits
   bridges into the `PumpConfiguration { tones: [PumpTone] }` shape
   `qcirc floquet` expects.
5. Short-circuit on empty pump_specs: when the upstream produces no
   processes (sparse Hamiltonian) the `/qcirc/floquet` handler returns
   an empty result instead of erroring, so the template's existing
   `[stage.condition]` blocks correctly skip the downstream regime_scan
   / constraints stages.
6. `qem_solve` (original stage 1) is left out of the template — it
   needs the qem-rs FEM stack on PATH and a substantial geometry input.
   This template starts from pre-computed S-parameters that the user
   passes via `--param s_params={…}`. Future iteration can prepend a
   `qem_solve` stage.

Smoke-tested with a zero S-params input: stages 1-4 produce real BBQ
output (empty arrays from the trivial S-params), stage 5 short-circuits,
stages 6-7 correctly skip via template conditions, stage 8 (`summary`)
produces `{"summary": {…}, "upstream": {…}}`.

## `parametric-process-design.toml` — ~~runtime-fails with 404~~ **FIXED 2026-06-02**

Moved back to the top-level `templates/` directory and runs end-to-end.

The fix landed all of:
- Built `quantum-qcirc/target/release/qcirc`.
- Added 7 `/qcirc/*` handlers in `quantum-api/main.rs` mirroring the
  `qchem`/`qatom`/`qspin` pattern. Each handler scans the request body
  for `<dep>_output` keys carrying the relevant marker fields (`modes`
  for a circuit, `feasible_points` for a regime result) so the
  orchestrator's automatic dep-wiring works without template `remap`.
- Added `"qcirc"` to `REQUIRED_TOOLS` and `quantum-qcirc/target/release`
  to `quantum-path.sh`.
- Enhanced `qorchestrate-cli`'s `--param key=value` parser to recognize
  JSON-shaped values (starts with `{` or `[`) so a netlist can be passed
  on the command line.
- Updated the template so the downstream stages declare their full
  data-dependency set (`pump_design`, `floquet`, `regime_scan` now
  depend on `qcirc_quantize` directly so the circuit is in their input
  map).

Smoke test with a transmon netlist correctly identifies a 4-photon
process at 17.28 GHz pump frequency. `floquet` and `circuit_constraints`
return null because the template's existing `[stage.condition]` blocks
correctly skip them when `pump_amplitude` isn't set — that's the
template's design, not a bug.

## Re-enable

Move the relevant file back to the parent `templates/` directory once
its dependency (handler routes + qcirc binary) is in place.
