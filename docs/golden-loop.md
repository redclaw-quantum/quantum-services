# Golden loop — design → fab → measure → re-design

`scripts/golden_loop.sh` drives the full QuantumRedClaw fabrication-handoff and
post-fab loop through the quantum-api REST gateway, end to end, on real services.
It's both a smoke test and a tutorial for how the fab pieces fit together.

## What it exercises

| # | Step | Endpoint | Proves |
|---|------|----------|--------|
| 1 | Discovery | `GET /foundry/profiles` | A foundry profile binds a PDK deck + junction recipe + layer map + DRC gating |
| 2 | GDS export | `POST /gds/export-chip` | Foundry-mapped, **framed** (alignment + dicing) and **dummy-filled** chip GDS + a **mask job-deck** |
| 3 | DRC | `POST /drc` | Layer-aware foundry PDK deck over the layout |
| 4 | JJ recipe | `POST /junction/recipe` | Ambegaokar–Baratoff nominal E_J / L_J / I_c |
| 5 | Yield | `POST /junction/yield` | Monte-Carlo yield on the **CUDA** GPU backend |
| 6 | Tolerance budget | `POST /junction/budget` | Reverse single-knob junction-σ budget (GPU MC) |
| 7 | Measurement | *(synthesized)* | A `CryoMeasurementRecord` standing in for a fridge run |
| 8 | Metrology ingest | `POST /qtwin/ingest` | Measured vs design → digital twin + **recalibration**, closing the loop back to QPUDIDP inverse design |

## Run it

```bash
# Needs a running quantum-api with the claw-gds / claw-yield(-gpu) / qtwin tools
# on PATH. On `tank`, put CUDA on PATH for the GPU yield:
export PATH=/usr/local/cuda/bin:$PATH LD_LIBRARY_PATH=/usr/local/cuda/lib64:$LD_LIBRARY_PATH

scripts/golden_loop.sh http://127.0.0.1:8765 ./golden-loop-out
# or against a custom port / foundry:
FOUNDRY=university_snf scripts/golden_loop.sh http://127.0.0.1:8799 /tmp/out
```

Artifacts land in the output dir: `chip.gds`, `drc.json`, `recipe.json`,
`yield.json`, `budget.json`, `twin.json`, plus the generated `design_*.json`
and `measurement.json`.

## Expected output (commercial_foundry)

```
1. discovery — pdk_deck=coplanar_foundry  junction_recipe=manhattan_alox_tight  layer_map=foundry_generic
2. GDS export — 761144 bytes  fill_tiles=11547  frame_marks=4
   job-deck (5 masks): cpw_etch(21) junction(30) alignment(60) dicing(61) dummy_fill(62)
3. DRC — deck=coplanar_foundry clean=True num_violations=0
4. JJ recipe — E_J=16.04 GHz  L_J=10.19 nH  I_c=32.3 nA  jσ=3.23%  (ambegaokar_baratoff)
5. yield — backend=cuda  yield=0.43
6. budget — backend=cuda  max_junction_σ=1.00%  recipe_σ=3.23%  meets=False  dominant=junction
7+8. metrology — chip status=Failed  critical=1  recal_suggestions=2
     q1 → RetunePulse: 6 MHz offset
     q2 → TriggerInverseDesign: QPUDIDP inverse design targeting f=5.150 GHz
```

## Notes

- The demo **deliberately** injects a defect: qubit 2 is measured ~60 MHz
  off-target so the metrology step trips a `Failed` deviation and emits a
  `TriggerInverseDesign` recalibration action — demonstrating the loop closing
  back to the design front (QPUDIDP). Drop that perturbation in
  `measurement.json` to see a clean return path.
- The budget reporting `meets=False` shows the `manhattan_alox_tight` recipe's
  junction tolerance (3.23%) is looser than the ~1% needed for 90% yield on this
  collision-prone 4-qubit chain — exactly the kind of go/no-go the platform is
  meant to surface before tape-out.
- This script covers the **fabrication-handoff + post-fab** loop, which runs on
  claw-gds / claw-yield / qtwin. It does not invoke the ML inverse-design front
  (QPUDIDP) or the full `design-to-chip` orchestration, which require those
  additional services; the twin's `TriggerInverseDesign` is where the two meet.
```
