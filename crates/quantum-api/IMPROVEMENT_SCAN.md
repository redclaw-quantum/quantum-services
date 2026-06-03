# Rust Improvement Scan: quantum-api
**Date:** 2026-04-28 11:05 UTC
**Rust:** 1.95 stable -- Edition 2024

## Changes Made

### Cargo.toml: rust-version added
- Added `rust-version = "1.95"`

### src/symclaw.rs (NEW FILE, 146 lines)
- Extracted 10 symclaw API handlers from main.rs
- Handlers: symclaw_health, symclaw_simplify, symclaw_differentiate, symclaw_integrate, symclaw_solve, symclaw_taylor, symclaw_limit, symclaw_codegen, symclaw_linalg, symclaw_polynomial, symclaw_analyze
- main.rs reduced by 137 lines

### src/cal.rs (NEW FILE, 140 lines)  
- Extracted 7 calibration API handlers from main.rs
- Handlers: cal_health, cal_spectroscopy, cal_rabi, cal_t1, cal_rb, cal_cycle_rb, cal_adaptive, cal_leakage_rb
- main.rs reduced by 113 lines

### Previously applied (PR #1, 2026-04-25)
- Edition 2021 → 2024, fixed broken path deps
- Extracted qcvv.rs (429 lines), various clippy fixes
- main.rs reduced from 6264 → 5342 lines

## Security Notes
cargo-audit not installed. No unsafe blocks. reqwest TLS-by-default.
Transitive dep claw-gds has one unused import warning (not in this repo).

## Files Over Limit (>1300 lines)
- src/main.rs: 5075 lines (still massively over limit)
  - Progress: 6264 → 5075 over two iterations
  - Requires systematic extraction of ~30 more domain modules
  - Next candidates: qfw.rs (~100L), transpile.rs (~90L), swap.rs (~250L), orchestrate.rs (~80L), scq.rs (~130L), bbq.rs (~140L), floquet.rs (~140L), qml.rs (~60L), cryo.rs (~60L), qnet.rs (~80L), qchem.rs (~80L), etc.

## Remaining Opportunities
1. Continue main.rs split: extract ~30 more domain modules (qfw, transpile, swap, scq, bbq, floquet, qml, cryo, qnet, qchem, wiring, qatom, pqec, qspin, qion, bosonic, codesign, qaoa, surgery, freq, xtalk, readout, bench, pipeline, mesh, gds, etc.)
2. 136 .unwrap() calls in production code
3. std::sync::Mutex for RESULT_CACHE (could use DashMap for concurrent perf)
