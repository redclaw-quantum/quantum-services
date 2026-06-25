#!/usr/bin/env bash
# golden_loop.sh — end-to-end design→fab→measure demo against a running quantum-api.
#
# Drives the full QuantumRedClaw fabrication-handoff + post-fab loop through the
# REST gateway, asserting each step and writing artifacts to an output dir:
#
#   1. discovery        — foundry profiles, layer maps, DRC decks, JJ recipes
#   2. GDS export       — foundry-mapped, framed (alignment+dicing), dummy-filled
#                         chip GDS + mask job-deck
#   3. DRC              — foundry PDK deck over the layout
#   4. JJ recipe        — Ambegaokar–Baratoff nominal E_J/L_J
#   5. yield (GPU)      — Monte-Carlo yield on the CUDA backend
#   6. tolerance budget — reverse single-knob junction-σ budget (GPU MC)
#   7. measurement      — synthesize a CryoMeasurementRecord (stands in for a
#                         fridge run measuring the fabricated chip)
#   8. metrology ingest — compare measured vs design → digital twin + recal
#
# Usage:  scripts/golden_loop.sh [API_URL] [OUT_DIR]
#   API_URL  default http://127.0.0.1:8765
#   OUT_DIR  default ./golden-loop-out
#
# Requires: a running quantum-api with the claw-gds / claw-yield(-gpu) / qtwin
# tools available (e.g. on `tank`, with CUDA on PATH for the GPU yield).
set -euo pipefail

API="${1:-http://127.0.0.1:8765}"
OUT="${2:-./golden-loop-out}"
mkdir -p "$OUT"

post() { curl -fsS -X POST "$API$1" -H 'content-type: application/json' -d "$2"; }
get()  { curl -fsS "$API$1"; }
hr()   { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

# Pick the foundry up front; everything downstream follows its profile.
FOUNDRY="${FOUNDRY:-commercial_foundry}"

hr "0. health"
get /gds/health >/dev/null && echo "quantum-api reachable at $API"

hr "1. discovery — foundry profile '$FOUNDRY'"
get /foundry/profiles > "$OUT/foundry_profiles.json"
read -r DECK RECIPE LAYERMAP GATE < <(python3 - "$OUT/foundry_profiles.json" "$FOUNDRY" <<'PY'
import sys, json
profiles = json.load(open(sys.argv[1]))["profiles"]
p = next(x for x in profiles if x["name"] == sys.argv[2])
print(p["pdk_deck"], p["junction_recipe"], p["layer_map"], p["gate_on_drc"])
PY
)
echo "  pdk_deck=$DECK  junction_recipe=$RECIPE  layer_map=$LAYERMAP  gate_on_drc=$GATE"

# Build the two design representations for the chip (geometry-level for the
# yield MC; spec-level for the digital twin) + a synthetic measurement.
python3 - "$OUT" <<'PY'
import json, sys, os
out = sys.argv[1]
n = 4
freqs = [5.0, 5.3, 5.15, 5.45]
def cl(f): return 300.0 - (f - 5.0) * 120.0  # cruder cross length ~ freq
# claw-yield NominalDesign (geometry + lj)
ny = {"qubits": [], "couplers": []}
for i in range(n):
    ny["qubits"].append({"id": i, "geometry": {"TransmonCross": {
        "cross_length": cl(freqs[i]), "cross_width": 30.0, "cross_gap": 6.0,
        "claw_length": 100.0, "claw_width": 10.0, "claw_gap": 6.0, "lj_nh": 10.0}},
        "target_freq_ghz": freqs[i], "anharmonicity_mhz": -300.0})
for a, b in [(0,1),(1,2),(2,3)]:
    ny["couplers"].append({"qubit_a": a, "qubit_b": b, "coupling_mhz": 2.0})
json.dump(ny, open(os.path.join(out, "design_nominal.json"), "w"))
# qtwin DesignSpec
ds = {"qubits": [], "couplers": []}
for i in range(n):
    ds["qubits"].append({"id": i, "target_freq_ghz": freqs[i], "predicted_t1_us": 100.0,
        "predicted_t2_us": 80.0, "anharmonicity_mhz": -300.0, "geometry": [],
        "predicted_dressed_params": None})
for a, b in [(0,1),(1,2),(2,3)]:
    ds["couplers"].append({"qubit_a": a, "qubit_b": b, "target_coupling_mhz": 2.0, "target_zz_khz": 50.0})
json.dump(ds, open(os.path.join(out, "design_spec.json"), "w"))
# Synthetic CryoMeasurementRecord: small perturbations + one bad qubit.
import math
meas = {"metadata": {"instrument_id": "VNA-01/AWG-03", "fridge_model": "Bluefors LD400",
    "measurement_date": "2026-06-24", "software_version": "qchar-2.1", "operator": "golden",
    "mixing_chamber_mk": 11.0}, "qubits": [], "couplers": []}
dfreq = [0.004, -0.006, 0.060, -0.003]  # q2 lands 60 MHz off → Critical
for i in range(n):
    meas["qubits"].append({"id": i, "freq_ghz": freqs[i] + dfreq[i], "t1_us": 88.0 - i*5,
        "t2_us": 70.0 - i*4, "anharmonicity_mhz": -298.0, "readout_fidelity": 0.985,
        "freq_sigma_mhz": 0.5})
for a, b in [(0,1),(1,2),(2,3)]:
    meas["couplers"].append({"qubit_a": a, "qubit_b": b, "coupling_mhz": 1.9, "zz_khz": 30.0, "gate_fidelity": 0.994})
json.dump(meas, open(os.path.join(out, "measurement.json"), "w"))
print("  wrote design_nominal.json, design_spec.json, measurement.json (%d qubits)" % n)
PY

hr "2. GDS export — foundry-mapped + framed + dummy-filled"
post /gds/export-chip "{\"cols\":2,\"rows\":2,\"layer_map\":\"$LAYERMAP\",\"tapeout_frame\":true,\"dummy_fill\":true}" > "$OUT/chip.json"
python3 - "$OUT/chip.json" "$OUT/chip.gds" <<'PY'
import sys, json
d = json.load(open(sys.argv[1]))
open(sys.argv[2], "wb").write(bytes.fromhex(d["hex"]))
fr = d.get("tapeout_frame") or {}
print("  GDS %d bytes  fill_tiles=%s  frame_marks=%s" % (d["n_bytes"], d.get("dummy_fill_tiles"), fr.get("alignment_marks")))
print("  job-deck (%d masks):" % d["job_deck"]["n_masks"])
for m in d["job_deck"]["masks"]:
    print("    L%d/%d %-12s %-8s %5d polys" % (m["gds_layer"], m["gds_datatype"], m["mask_name"], m["polarity"], m["n_polygons"]))
PY

hr "3. DRC — PDK deck '$DECK'"
post /drc "{\"cols\":2,\"rows\":2,\"pdk\":\"$DECK\"}" > "$OUT/drc.json"
python3 -c "import json;d=json.load(open('$OUT/drc.json'));print('  deck=%s clean=%s num_violations=%d'%(d['deck'],d['clean'],d['num_violations']))"

hr "4. JJ recipe '$RECIPE'"
post /junction/recipe "{\"recipe\":\"$RECIPE\"}" > "$OUT/recipe.json"
python3 -c "import json;d=json.load(open('$OUT/recipe.json'));e=d['eval'];print('  E_J=%.2f GHz  L_J=%.2f nH  I_c=%.1f nA  jσ=%.2f%%  (%s)'%(e['ej_ghz'],e['lj_nh'],e['ic_ua']*1000,e['junction_sigma_percent'],e['ej_source']))"

hr "5. yield — Monte Carlo (GPU)"
DESIGN_NOM=$(cat "$OUT/design_nominal.json")
post /junction/yield "{\"design\":$DESIGN_NOM,\"recipe\":\"$RECIPE\",\"samples\":60000,\"collision_threshold_mhz\":40.0}" > "$OUT/yield.json"
python3 -c "import json;d=json.load(open('$OUT/yield.json'));print('  backend=%s  yield=%.4f'%(d['backend'],d['yield']['yield_fraction']))"

hr "6. tolerance budget — reverse junction-σ (GPU)"
post /junction/budget "{\"design\":$DESIGN_NOM,\"recipe\":\"$RECIPE\",\"target_yield\":0.9,\"samples\":40000}" > "$OUT/budget.json"
python3 -c "import json;d=json.load(open('$OUT/budget.json'));b=d['budget'];print('  backend=%s  max_junction_σ=%.2f%%  recipe_σ=%.2f%%  meets=%s  dominant=%s'%(d['backend'],b['max_junction_sigma_pct'],b['recipe_junction_sigma_pct'],b['meets_target'],b['dominant_contributor']))"

hr "7+8. metrology ingest — measured vs design → twin + recalibration"
DESIGN_SPEC=$(cat "$OUT/design_spec.json")
MEAS=$(cat "$OUT/measurement.json")
post /qtwin/ingest "{\"measurement\":$MEAS,\"design\":$DESIGN_SPEC,\"recalibrate\":true}" > "$OUT/twin.json"
python3 - "$OUT/twin.json" <<'PY'
import sys, json
d = json.load(open(sys.argv[1]))
s = d["summary"]
print("  chip status=%s  deviations=%d  critical=%d  recal_suggestions=%d"
      % (s["overall_status"], s["n_deviations"], s["critical_count"], len(d["recalibration"])))
for r in d["recalibration"][:4]:
    act = r["action"]
    name = act if isinstance(act, str) else list(act.keys())[0]
    print("    q%s → %s: %s" % (r["qubit_id"], name, r["expected_improvement"]))
PY

hr "done"
echo "Artifacts in $OUT/ : chip.gds, drc.json, recipe.json, yield.json, budget.json, twin.json"
echo "Full design→fab→measure loop exercised end-to-end."
