# quantum-api

HTTP microservice gateway for the QuantumRedClaw platform. Wraps quantum CLI tools (rustypulse, rustystim, rustyfreq, rustyxtalk, rustybench-q, qstar-rs, rustybbq, rustyfloquet, and 20+ others) behind a uniform REST API consumed by the ZeroClaw gateway (`QuantumTool`).

**Port:** `8765`
**Binary:** `/usr/local/bin/quantum-api`
**Systemd:** `quantum-api.service`
**Stack:** Rust · Axum · tokio · serde_json

---

## Quick start

```bash
# Build and install
cargo build --release
sudo cp target/release/quantum-api /usr/local/bin/

# Run directly
quantum-api --port 8765

# Via systemd
sudo systemctl start quantum-api
sudo systemctl status quantum-api

# Health check
curl http://127.0.0.1:8765/health
```

---

## Endpoint reference

All `POST` endpoints accept and return `application/json`. `GET /*/health` endpoints return `{"status":"ok","service":"<name>"}`.

### Core

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Service health |

### qtwin — Calibration twin

| Method | Path | Description |
|--------|------|-------------|
| POST | `/qtwin/compare` | Compare chip state against design spec |
| GET | `/qtwin/:chip/compare` | Chip-scoped compare |
| POST | `/qtwin/recalibrate` | Trigger recalibration via QPUDIDP/qem |
| POST | `/qtwin/qec-update` | Push QEC threshold update |
| POST | `/qtwin/mock` | Mock chip state for testing |

### freq — Frequency optimization

| Method | Path | Description |
|--------|------|-------------|
| POST | `/freq/optimize` | Optimal qubit frequency assignment |
| POST | `/freq/check` | Check frequency collisions |
| POST | `/freq/yield` | Frequency yield estimation |

### xtalk — Crosstalk analysis

| Method | Path | Description |
|--------|------|-------------|
| POST | `/xtalk/coupling` | Coupling strength matrix |
| POST | `/xtalk/zz` | ZZ crosstalk (full) |
| POST | `/xtalk/zz-simple` | ZZ crosstalk (analytic approximation) |
| POST | `/xtalk/crosstalk` | Multi-qubit crosstalk sweep |
| POST | `/xtalk/simulate` | Time-domain crosstalk simulation |

### readout — Readout design

| Method | Path | Description |
|--------|------|-------------|
| GET | `/readout/health` | Health |
| POST | `/readout/design` | Readout resonator design |
| POST | `/readout/multiplex` | Multiplexed readout layout |
| POST | `/readout/optimize` | Readout parameter optimization |
| POST | `/readout/fidelity` | Readout fidelity assessment |

### bench — Benchmarking

| Method | Path | Description |
|--------|------|-------------|
| GET | `/bench/health` | Health |
| POST | `/bench/predict` | Predict benchmark scores |
| POST | `/bench/suggest` | Suggest parameter improvements |
| POST | `/bench/qv` | Quantum volume estimate |
| POST | `/bench/rb` | Randomized benchmarking |
| POST | `/bench/compare` | Multi-chip benchmark comparison |

### qstar — QEC thresholds

| Method | Path | Description |
|--------|------|-------------|
| POST | `/qstar/threshold` | Error correction threshold via qstar-rs |

### surgery — Lattice surgery

| Method | Path | Description |
|--------|------|-------------|
| GET | `/surgery/health` | Health |
| POST | `/surgery/resources` | Resource estimation |
| POST | `/surgery/factory` | Magic state factory design |
| POST | `/surgery/compile` | Lattice surgery compilation |
| POST | `/surgery/visualize` | Lattice surgery visualization |

### pulse — Pulse simulation

| Method | Path | Description |
|--------|------|-------------|
| POST | `/pulse/simulate` | Pulse-level gate simulation (rustypulse) |

### stim — Stabilizer circuits

| Method | Path | Description |
|--------|------|-------------|
| POST | `/stim/gen` | Generate stabilizer circuit |
| POST | `/stim/circuit` | Run Stim circuit simulation |
| POST | `/stim/ldpc` | LDPC code analysis |
| POST | `/stim/xzzx` | XZZX surface code |

### qexplore — Parameter sweep

| Method | Path | Description |
|--------|------|-------------|
| POST | `/qexplore/sweep` | Multi-parameter QPU sweep |
| POST | `/qexplore/fridge` | Dilution refrigerator parameter sweep |

### pipeline — Design pipeline

| Method | Path | Description |
|--------|------|-------------|
| POST | `/pipeline/design` | Full design pipeline: inverse → surrogate → freq → bench |

### qpudidp — QPUDIDP surrogate models

| Method | Path | Description |
|--------|------|-------------|
| POST | `/qpudidp/inverse-design-rmflow` | Inverse design via normalizing flow |
| POST | `/qpudidp/paired-design-predict` | Paired design prediction |
| POST | `/qpudidp/rectangular-cavity-3d-predict` | 3D rectangular cavity surrogate |
| POST | `/qpudidp/uncertainty-quantile` | Uncertainty quantile estimation |

### qem — Electromagnetic solver

| Method | Path | Description |
|--------|------|-------------|
| POST | `/qem/solve_lom` | Lumped oscillator model solve |
| POST | `/qem/solve_lom_tunable` | Tunable transmon LOM |
| POST | `/qem/solve_lom_cavity` | Cavity-coupled LOM |

### bbq — Black-box quantization

| Method | Path | Description |
|--------|------|-------------|
| GET | `/bbq/health` | Health |
| POST | `/bbq/quantize` | BBQ quantization of an EM component |
| POST | `/bbq/bus` | Bus coupling quantization |
| POST | `/bbq/hamiltonian` | Effective Hamiltonian extraction |
| POST | `/bbq/zz-coupling` | ZZ coupling from BBQ |
| POST | `/bbq/coupler-zz` | Coupler-mediated ZZ |
| POST | `/bbq/jpa-model` | JPA parametric amplifier model |

### floquet — Floquet / Lindblad dynamics

| Method | Path | Description |
|--------|------|-------------|
| GET | `/floquet/health` | Health |
| POST | `/floquet/spectrum` | Floquet quasi-energy spectrum |
| POST | `/floquet/propagator` | Stroboscopic propagator |
| POST | `/floquet/lindblad` | Lindblad master equation integration |
| POST | `/floquet/bbq-floquet` | BBQ + Floquet combined analysis |
| POST | `/floquet/grape` | GRAPE optimal control pulse design |
| POST | `/floquet/grape-su2` | SU(2) GRAPE variant |
| POST | `/floquet/flime-solve` | FLiME stroboscopic map (RK4 superoperator, Schur eigendecomp, power-iteration steady state) |

### cal — Calibration sequences

| Method | Path | Description |
|--------|------|-------------|
| GET | `/cal/health` | Health |
| POST | `/cal/spectroscopy` | Qubit spectroscopy |
| POST | `/cal/rabi` | Rabi oscillation calibration |
| POST | `/cal/t1` | T1 relaxation measurement |
| POST | `/cal/rb` | Randomized benchmarking calibration |
| POST | `/cal/cycle-rb` | Cycle benchmarking |
| POST | `/cal/adaptive` | Adaptive calibration sequence |
| POST | `/cal/leakage-rb` | Leakage randomized benchmarking |

### qfw — Quantum firmware / pulse compilation

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qfw/health` | Health |
| POST | `/qfw/compile` | Compile gate sequence to pulses |
| POST | `/qfw/schedule` | Schedule pulse sequence |
| POST | `/qfw/simulate` | Simulate compiled pulses |
| POST | `/qfw/export` | Export pulse program |

### transpile — Circuit transpilation

| Method | Path | Description |
|--------|------|-------------|
| GET | `/transpile/health` | Health |
| POST | `/transpile/compile` | Transpile circuit to native gates |
| POST | `/transpile/analyze` | Circuit depth / gate count analysis |
| POST | `/transpile/noise-aware` | Noise-aware routing and transpilation |
| POST | `/transpile/compare` | Compare transpilation strategies |
| POST | `/transpile/xtalk-map` | Crosstalk-aware qubit mapping |

### qml — Quantum machine learning

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qml/health` | Health |
| POST | `/qml/classify` | Quantum classifier inference |
| POST | `/qml/kernel` | Quantum kernel evaluation |
| POST | `/qml/resources` | QML resource estimation |
| POST | `/qml/barren-plateau` | Barren plateau analysis |
| POST | `/qml/readout-crosstalk` | Readout crosstalk characterization |

### cryo — Cryogenic systems

| Method | Path | Description |
|--------|------|-------------|
| GET | `/cryo/health` | Health |
| POST | `/cryo/analyze` | Cryostat thermal model analysis |
| POST | `/cryo/power` | Power budget estimation |
| POST | `/cryo/compare` | Compare cryostat configurations |
| POST | `/cryo/scale` | Scale cooling power with qubit count |

### qnet — Quantum networking

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qnet/health` | Health |
| POST | `/qnet/analyze` | Quantum network analysis |
| POST | `/qnet/entangle` | Entanglement distribution modeling |
| POST | `/qnet/scale` | Network scalability analysis |
| POST | `/qnet/compare-links` | Compare quantum link technologies |

### qchem — Quantum chemistry

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qchem/health` | Health |
| POST | `/qchem/molecule` | Molecular Hamiltonian construction |
| POST | `/qchem/vqe` | VQE resource estimation |
| POST | `/qchem/resources` | Full quantum chemistry resource estimate |

### wiring — Cryogenic wiring

| Method | Path | Description |
|--------|------|-------------|
| GET | `/wiring/health` | Health |
| POST | `/wiring/design` | Cryogenic wiring harness design |
| POST | `/wiring/noise` | Wiring noise analysis |
| POST | `/wiring/scale` | Scale wiring with qubit count |
| POST | `/wiring/optimize` | Optimize wiring layout |

### extract — Parasitic extraction

| Method | Path | Description |
|--------|------|-------------|
| GET | `/extract/health` | Health |
| POST | `/extract/cpw` | CPW resonator parasitic extraction |
| POST | `/extract/tls` | Two-level system loss extraction |

### qatom — Neutral atom qubits

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qatom/health` | Health |
| POST | `/qatom/design` | Neutral atom qubit design |
| POST | `/qatom/gate` | Rydberg gate fidelity |
| POST | `/qatom/blockade` | Rydberg blockade radius |
| POST | `/qatom/loading` | Atom loading probability |
| POST | `/qatom/multi-gate` | Multi-qubit gate optimization |
| POST | `/qatom/zone-layout` | Zoned array layout |

### pqec — Pulse-level QEC

| Method | Path | Description |
|--------|------|-------------|
| GET | `/pqec/health` | Health |
| POST | `/pqec/assess` | QEC code assessment |
| POST | `/pqec/threshold` | Fault-tolerance threshold |
| POST | `/pqec/overhead` | Logical qubit overhead |
| POST | `/pqec/sweep` | Error model parameter sweep |

### qspin — Spin qubits

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qspin/health` | Health |
| POST | `/qspin/design` | Spin qubit device design |
| POST | `/qspin/fidelity` | Gate fidelity estimation |
| POST | `/qspin/stability` | Charge noise stability analysis |
| POST | `/qspin/fab` | Fabrication tolerance analysis |
| POST | `/qspin/yield` | Device yield estimation |
| POST | `/qspin/valley-split` | Valley splitting analysis |
| POST | `/qspin/nuclear-bath` | Nuclear spin bath decoherence |

### qion — Trapped ion qubits

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qion/health` | Health |
| POST | `/qion/design` | Ion trap design |
| POST | `/qion/ms-gate` | Mølmer-Sørensen gate fidelity |
| POST | `/qion/modes` | Normal mode spectrum |
| POST | `/qion/cooling` | Sideband cooling analysis |
| POST | `/qion/schedule` | Gate schedule optimization |
| POST | `/qion/raman-cool` | Raman cooling simulation |

### bosonic — Bosonic qubits

| Method | Path | Description |
|--------|------|-------------|
| GET | `/bosonic/health` | Health |
| POST | `/bosonic/simulate` | Bosonic code simulation (cat/GKP/binomial) |
| POST | `/bosonic/compare` | Compare bosonic encodings |
| POST | `/bosonic/optimize` | Optimize bosonic code parameters |
| POST | `/bosonic/break-even` | Break-even point analysis |
| POST | `/bosonic/concat` | Concatenated bosonic code |

### codesign — Hardware–software co-design

| Method | Path | Description |
|--------|------|-------------|
| GET | `/codesign/health` | Health |
| POST | `/codesign/optimize` | Co-design optimization |
| POST | `/codesign/roadmap` | Scaling roadmap generation |
| POST | `/codesign/compare-platforms` | Compare quantum hardware platforms |
| POST | `/codesign/what-if` | What-if scenario analysis |
| POST | `/codesign/sensitivity` | Sensitivity analysis |

### qaoa — Quantum optimization

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qaoa/health` | Health |
| POST | `/qaoa/maxcut` | QAOA MaxCut |
| POST | `/qaoa/portfolio` | Portfolio optimization |
| POST | `/qaoa/tsp` | Travelling salesman problem |
| POST | `/qaoa/resources` | QAOA resource estimation |

### pkg — Chip packaging

| Method | Path | Description |
|--------|------|-------------|
| GET | `/pkg/health` | Health |
| POST | `/pkg/design` | Package design (STEP/GDS/STL/DXF) |
| POST | `/pkg/box-modes` | Box mode spectrum |
| POST | `/pkg/wirebonds` | Wire bond inductance model |
| POST | `/pkg/export` | Export package geometry |

### swap — Lindblad SWAP simulation

Based on Mollenhauer et al. arXiv:2407.16743.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/swap/health` | Health |
| POST | `/swap/figure1d` | Fig 1d reproduction |
| POST | `/swap/figure3a` | Fig 3a reproduction |
| POST | `/swap/figure3c` | Fig 3c reproduction |
| POST | `/swap/figure4c` | Fig 4c reproduction |
| POST | `/swap/fock-convergence` | Fock space truncation convergence |
| POST | `/swap/sw-validity` | Schrieffer-Wolff validity bounds |
| POST | `/swap/nmodule-chain` | N-module chain simulation |
| POST | `/swap/tls-loss` | TLS-induced loss analysis |
| POST | `/swap/chi-sensitivity` | χ sensitivity to cable placement (±0.5 mm → 0.4–9.2 MHz spread) |
| POST | `/swap/spam-model` | SPAM confusion matrix + 3-fidelity budget |
| POST | `/swap/crosstalk-sweep` | ZZ crosstalk sweep |
| POST | `/swap/param-spread` | Monte Carlo parameter spread |
| POST | `/swap/nmodule-scaling` | N-module scaling analysis |

### oqfp — OQFP chip spec management

| Method | Path | Description |
|--------|------|-------------|
| GET | `/oqfp/health` | Health |
| POST | `/oqfp/validate` | Validate OQFP spec |
| POST | `/oqfp/summary` | Spec summary |
| POST | `/oqfp/diff` | Diff two specs |
| POST | `/oqfp/create` | Create new OQFP spec |

### scq — superconducting qubit spectra

| Method | Path | Description |
|--------|------|-------------|
| GET | `/scq/health` | Health |
| POST | `/scq/spectrum` | Transmon energy spectrum |
| POST | `/scq/dispersion` | Dispersive shift |
| POST | `/scq/flux-sweep` | Flux-dependent spectrum sweep |
| POST | `/scq/coherence` | T1/T2 coherence estimates |

### orchestrate — Workflow orchestration

Proxies to `qorchestrate:8767`.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/orchestrate/health` | Health |
| GET | `/orchestrate/stages` | List available pipeline stages |
| POST | `/orchestrate/validate` | Validate workflow DAG |
| POST | `/orchestrate/run` | Execute workflow |
| POST | `/orchestrate/xeb-verify` | XEB verification workflow |

### symclaw — Symbolic mathematics

| Method | Path | Description |
|--------|------|-------------|
| GET | `/symclaw/health` | Health |
| POST | `/symclaw/simplify` | Symbolic simplification |
| POST | `/symclaw/differentiate` | Symbolic differentiation |
| POST | `/symclaw/integrate` | Symbolic integration |
| POST | `/symclaw/solve` | Algebraic solver |
| POST | `/symclaw/taylor` | Taylor series expansion |
| POST | `/symclaw/limit` | Limit evaluation |
| POST | `/symclaw/codegen` | Code generation from expression |
| POST | `/symclaw/linalg` | Symbolic linear algebra |
| POST | `/symclaw/polynomial` | Polynomial analysis |
| POST | `/symclaw/analyze` | General expression analysis |

### mesh — FEM mesh generation

Integrated directly (path deps, no subprocess).

| Method | Path | Description |
|--------|------|-------------|
| GET | `/mesh/health` | Health |
| POST | `/mesh/transmon-cross` | Cross-shaped transmon mesh |
| POST | `/mesh/rectangular-cavity-3d` | 3D rectangular cavity mesh |
| POST | `/mesh/tunable-transmon` | Tunable transmon mesh |
| POST | `/mesh/xmon` | Xmon qubit mesh |
| POST | `/mesh/fluxonium` | Fluxonium mesh |
| POST | `/mesh/cpw-resonator` | CPW resonator mesh |
| POST | `/mesh/chip` | Full chip mesh assembly |
| POST | `/mesh/quality` | Mesh quality metrics |

### gds — GDS layout export

| Method | Path | Description |
|--------|------|-------------|
| GET | `/gds/health` | Health |
| POST | `/gds/transmon-cross` | Transmon cross GDS layout |
| POST | `/gds/rectangular-cavity-3d` | 3D cavity GDS export |
| POST | `/gds/chip-layout` | Full chip GDS layout |
| POST | `/gds/export` | Export GDS file |

### clawview — Field visualization

Proxies to clawview service on port 9090.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/clawview/health` | Health |
| GET | `/clawview/participation` | Participation ratios |
| POST | `/clawview/streamlines` | Field streamlines |
| GET | `/clawview/isosurface` | Isosurface data |
| GET | `/clawview/coupling` | Coupling field visualization |
| POST | `/clawview/surrogate/predict` | Surrogate model field prediction |
| POST | `/clawview/cross-section` | Cross-section field view |
| POST | `/clawview/layout/from-params` | Layout visualization from parameters |
| GET | `/clawview/formats` | Supported output formats |

### qcvv — Quantum Characterization, Verification, and Validation

Pure analytical Rust — no subprocess or external service required.

| Method | Path | Description |
|--------|------|-------------|
| GET | `/qcvv/health` | Health |
| POST | `/qcvv/quantum-volume` | Quantum Volume via HOP model (depolarising + T1/T2 + readout) |
| POST | `/qcvv/process-fidelity` | Process fidelity: F_avg → F_process → diamond distance |
| POST | `/qcvv/zne` | Zero-Noise Extrapolation (Richardson/exponential; Neville's algorithm for n>2 scales) |
| POST | `/qcvv/clops` | CLOPS (Circuit Layer Operations Per Second) |
| POST | `/qcvv/rb-analysis` | RB analysis: depolarising parameter, EPC, interleaved RB |

**Example — Quantum Volume:**
```bash
curl -s -X POST http://127.0.0.1:8765/qcvv/quantum-volume \
  -H 'Content-Type: application/json' \
  -d '{
    "n_qubits": 5,
    "t1_us": 100.0,
    "t2_us": 80.0,
    "gate_error": 0.001,
    "readout_error": 0.01,
    "gate_time_ns": 50.0
  }'
# → {"quantum_volume": 32, "max_passing_n": 5, "hop_values": [...]}
```

---

## Architecture notes

- All endpoints that wrap CLI tools spawn the appropriate binary as a subprocess, pass JSON on stdin, and parse JSON from stdout.
- Endpoints in the `mesh`, `gds`, and `pkg` groups use in-process Rust crates (path dependencies) — no subprocess.
- `clawview` and `orchestrate` are reverse proxies to separate services.
- Response caching is applied selectively on read-heavy endpoints (e.g., `qtwin/compare`).
- All handlers are instrumented with `tracing` spans; set `RUST_LOG=quantum_api=debug` for verbose output.

## Data files

| File | Purpose |
|------|---------|
| `/nvme/quantum/data/designs/chip-v1.oqfp.json` | OQFP spec for 20-qubit transmon chip |
| `/nvme/quantum/data/brains/lab3-qpu.brain` | clawhdf5 memory: calibration + chip registry + alerts |
