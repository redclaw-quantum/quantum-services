# CLAUDE.md — quantum-api

HTTP gateway that wraps all Tier 1 quantum CLI tools as JSON endpoints. Used by ZeroClaw skills to execute quantum tool calls from conversational queries.

## Port

Default: **8765**. Override with `QUANTUM_API_PORT` env var.

## Build & Run

```bash
# Build
cargo build --release

# Run (dev)
QUANTUM_API_PORT=8765 ./target/release/quantum-api

# Run as service
sudo systemctl start quantum-api
sudo systemctl enable quantum-api
```

## Endpoints

### Core / qtwin
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/health` | — | — |
| POST | `/qtwin/compare` | `qtwin compare` | `{design:{}, measured:{}}` |
| GET | `/qtwin/:chip/compare` | `qtwin compare` | chip slug |
| POST | `/qtwin/recalibrate` | `qtwin recalibrate` | `{twin:{}}` |
| POST | `/qtwin/qec-update` | `qtwin qec-update` | `{twin:{}, code:"surface"}` |
| POST | `/qtwin/mock` | `qtwin mock` | `{design:{}, sigma:15.0}` |

### Frequency / Crosstalk
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| POST | `/freq/optimize` | `freq optimize` | `{topology:"heavy_hex", rows:3, cols:3}` |
| POST | `/freq/check` | `freq check` | `{topology:{}, assignments:[]}` |
| POST | `/freq/yield` | `freq yield` | `{topology:"heavy_hex", sigma:10.0, samples:10000}` |
| POST | `/xtalk/coupling` | `xtalk coupling` | `{layout:{}}` |
| POST | `/xtalk/zz` | `xtalk zz` | `{layout:{}}` |
| POST | `/xtalk/zz-simple` | `xtalk zz-simple` | `{layout:{}}` |
| POST | `/xtalk/crosstalk` | `xtalk crosstalk` | `{layout:{}, drive_qubit:7}` |
| POST | `/xtalk/simulate` | `xtalk simulate` | `{layout:{}, gate:"cx", qubits:"0,1"}` |

### Readout / Surgery
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/readout/health` | — | — |
| POST | `/readout/design` | `readout design` | `{qubit_freq:5.0, anharmonicity:-250.0, target_fidelity:0.999}` |
| POST | `/readout/multiplex` | `readout multiplex` | `{qubit_freqs:[4.8,5.0,5.1], feedlines:1}` |
| POST | `/readout/optimize` | `readout optimize` | `{qubit_freq:5.0, t1:80.0, target_fidelity:0.999}` |
| POST | `/readout/fidelity` | `readout fidelity` | `{chi_mhz:1.5, kappa_mhz:0.5, t1_us:80.0, integration_time_ns:500}` |
| GET | `/surgery/health` | — | — |
| POST | `/surgery/resources` | `surgery resources` | `{circuit:{}, distances:[3,5,7]}` |
| POST | `/surgery/factory` | `surgery factory` | `{protocol:"15to1", distance:7, target_error:1e-10}` |
| POST | `/surgery/compile` | `surgery compile` | `{circuit:{}, distance:5}` |
| POST | `/surgery/visualize` | `surgery visualize` | `{schedule:{}}` |

### Bench / QStar / QExplore / Pipeline
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| POST | `/bench/predict` | `bench predict` | `{n_qubits:20, t1:80, t2:60, gate_fidelity:0.9987, readout_fidelity:0.997}` |
| POST | `/bench/suggest` | `bench suggest` | same as predict |
| POST | `/qstar/threshold` | `qstar threshold` | `{code:"surface", distances:"3,5,7", shots:1000}` |
| POST | `/qexplore/sweep` | `qexplore sweep` | `{n_qubits:"20,50", topology:"heavy_hex", budget:"research"}` |
| POST | `/qexplore/fridge` | `qexplore fridge` | `{n_qubits:100, topology:"heavy_hex"}` |
| POST | `/pipeline/design` | internal pipeline | `{device_type, qubit_frequency_ghz, anharmonicity_mhz}` |
| POST | `/pulse/simulate` | `rustypulse simulate` | `{hamiltonian:{}, pulse:{}, dt_ns:0.1}` |
| POST | `/stim/gen` | `rustystim gen` | `{code:"surface", distance:3, rounds:10}` |
| POST | `/stim/circuit` | `rustystim circuit` | `{circuit_str:"..."}` |

### BBQ / Floquet
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/bbq/health` | — | — |
| POST | `/bbq/quantize` | `rustybbq quantize` | `{s_params:{}, junction_port_indices:[], ec:[0.3]}` |
| POST | `/bbq/bus` | `rustybbq bus` | `{z0:50, length:0.1, freq_start:4, freq_stop:8}` |
| POST | `/bbq/hamiltonian` | `rustybbq hamiltonian` | `{modules:[], interactions:[], n_evals:20}` |
| GET | `/floquet/health` | — | — |
| POST | `/floquet/spectrum` | `rustyfloquet spectrum` | `{hamiltonian:{}, n_harmonics:5}` |
| POST | `/floquet/propagator` | `rustyfloquet propagator` | `{hamiltonian:{}, dt_ns:0.01, method:"rk4"}` |
| POST | `/floquet/lindblad` | `rustyfloquet lindblad` | `{hamiltonian:{}, t1_us:100.0, t_phi_us:80.0, n_periods:100}` |
| POST | `/floquet/bbq-floquet` | `rustyfloquet bbq-floquet` | `{hamiltonian:{MultiModuleHamiltonian}, drive_freq:5.0, drive_amp:0.01, n_harmonics:3}` |
| POST | `/floquet/grape` | `rustyfloquet floquet-grape` | `{hamiltonian:{}, target:"X", steps:100, duration:40.0, iterations:500}` |

### Cal / QFW / Transpile
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/cal/health` | — | — |
| POST | `/cal/spectroscopy` | `rustycal spectroscopy` | `{freq_start:4.5, freq_stop:5.5, points:100}` |
| POST | `/cal/rabi` | `rustycal rabi` | `{amp_start:0.0, amp_stop:1.0, points:50}` |
| POST | `/cal/t1` | `rustycal t1` | `{max_delay:100.0, points:100}` |
| POST | `/cal/rb` | `rustycal rb` | `{max_cliffords:200, sequences:50, points:20}` |
| GET | `/qfw/health` | — | — |
| POST | `/qfw/compile` | `qfw compile` | `{circuit:{}, calibration:{}}` |
| POST | `/qfw/schedule` | `qfw schedule` | `{circuit:{}, dd:"xy4"}` |
| POST | `/qfw/simulate` | `qfw simulate` | `{schedule:{}, shots:1000}` |
| POST | `/qfw/export` | `qfw export` | `{schedule:{}, format:"openqasm3"}` |
| GET | `/transpile/health` | — | — |
| POST | `/transpile/compile` | `transpile compile` | `{circuit:{}, target:"ibm_heavy_hex", optimization:2}` |
| POST | `/transpile/analyze` | `transpile analyze` | `{circuit:{}}` |
| POST | `/transpile/noise-aware` | `transpile noise-aware` | `{circuit:{}, noise:{}, target:"ibm_heavy_hex"}` |
| POST | `/transpile/compare` | `transpile compare` | `{circuit:{}, targets:"ibm,google,linear"}` |

### Packaging / QEM / QPUDIDP
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/pkg/health` | — | — |
| POST | `/pkg/design` | `rustypkg design` | `{housing_length_mm:50, housing_width_mm:40, housing_height_mm:15, n_sma_ports:4}` |
| POST | `/pkg/box-modes` | `rustypkg box-modes` | `{housing_length_mm:50, housing_width_mm:40, housing_height_mm:15}` |
| POST | `/pkg/wirebonds` | `rustypkg wirebonds` | `{chip:{}, substrate:{}}` |
| POST | `/pkg/export` | `rustypkg export` | `{design:{}, format:"step"\|"gds"\|"stl"\|"dxf"}` |
| POST | `/qem/solve_lom` | qem:8430 proxy | `{geometry:{}, qubit_port:0}` |
| POST | `/qem/solve_lom_tunable` | qem:8430 proxy | `{geometry:{}, squid_port:0}` |
| POST | `/qem/solve_lom_cavity` | qem:8430 proxy | `{geometry:{}, n_modes:5}` |
| POST | `/qpudidp/inverse-design-rmflow` | qpudidp:8420 proxy | `{device_type, qubit_frequency_ghz, anharmonicity_mhz, [linewidth_khz, n_candidates]}` |
| POST | `/qpudidp/paired-design-predict` | qpudidp:8420 proxy | `{cross_length, cross_width, cross_gap, claw_length, claw_width, claw_gap, lj_nh, res_length_um}` |
| POST | `/qpudidp/rectangular-cavity-3d-predict` | qpudidp:8420 proxy | `{length_mm, width_mm, height_mm, [material_id]}` |
| POST | `/qpudidp/uncertainty-quantile` | qpudidp:8420 proxy | `{device_type, params:[f64], [q_low, q_high, n_samples]}` |

### QML / Cryo / QNet
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/qml/health` | — | — |
| POST | `/qml/classify` | `rustyqml classify` | `{data:[], n_classes:2, circuit:"vqc"}` |
| POST | `/qml/kernel` | `rustyqml kernel` | `{x_train:[], x_test:[], kernel:"fidelity"}` |
| POST | `/qml/resources` | `rustyqml resources` | `{n_features:10, n_classes:4}` |
| POST | `/qml/barren-plateau` | `rustyqml barren-plateau` | `{n_qubits:8, depth:10}` |
| GET | `/cryo/health` | — | — |
| POST | `/cryo/analyze` | `rustycryo analyze` | `{fridge_type:"dilution", n_qubits:100}` |
| POST | `/cryo/power` | `rustycryo power` | `{stages:[]}` |
| POST | `/cryo/compare` | `rustycryo compare` | `{fridges:["dilution","adiabatic"]}` |
| POST | `/cryo/scale` | `rustycryo scale` | `{n_qubits_list:[100,1000,10000]}` |
| GET | `/qnet/health` | — | — |
| POST | `/qnet/analyze` | `rustyqnet analyze` | `{topology:"star", n_nodes:5, link_type:"fiber"}` |
| POST | `/qnet/entangle` | `rustyqnet entangle` | `{nodes:[0,1], protocol:"bbm92"}` |
| POST | `/qnet/scale` | `rustyqnet scale` | `{n_nodes_list:[10,100,1000]}` |
| POST | `/qnet/compare-links` | `rustyqnet compare-links` | `{link_types:["fiber","free_space","satellite"]}` |

### QChem / Wiring / Extract
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/qchem/health` | — | — |
| POST | `/qchem/molecule` | `rustyqchem molecule` | `{molecule:"H2", basis:"sto-3g"}` |
| POST | `/qchem/vqe` | `rustyqchem vqe` | `{molecule:"H2", ansatz:"uccsd", layers:2}` |
| POST | `/qchem/resources` | `rustyqchem resources` | `{molecule:"LiH", basis:"sto-3g"}` |
| GET | `/wiring/health` | — | — |
| POST | `/wiring/design` | `rustycryo-wiring design` | `{n_qubits:20, fridge_type:"dilution"}` |
| POST | `/wiring/noise` | `rustycryo-wiring noise` | `{wiring:{}}` |
| POST | `/wiring/scale` | `rustycryo-wiring scale` | `{n_qubits_list:[20,100,1000]}` |
| POST | `/wiring/optimize` | `rustycryo-wiring optimize` | `{wiring:{}, target:"heat_load"}` |
| GET | `/extract/health` | — | — |
| POST | `/extract/cpw` | `rustyextract cpw` | `{width_um:10.0, gap_um:6.0, substrate:"Si", thickness_um:200}` |
| POST | `/extract/tls` | `rustyextract tls` | `{cpw:{}, participation_ratio:0.01}` |

### QAtom / PQEC / QSpin / QIon
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/qatom/health` | — | — |
| POST | `/qatom/design` | `qatom design` | `{platform:"rydberg", n_qubits:100, array:"square"}` |
| POST | `/qatom/gate` | `qatom gate` | `{gate:"cz", species:"Rb87"}` |
| POST | `/qatom/blockade` | `qatom blockade` | `{species:"Rb87", principal_n:60}` |
| POST | `/qatom/loading` | `qatom loading` | `{array:{}, trap_depth_uk:1000}` |
| POST | `/qatom/multi-gate` | `qatom multi-gate` | `{circuit:[], array:{}}` |
| GET | `/pqec/health` | — | — |
| POST | `/pqec/assess` | `pqec assess` | `{code:"surface", distance:5, t1_us:100.0}` |
| POST | `/pqec/threshold` | `pqec threshold` | `{code:"surface", distances:"3,5,7"}` |
| POST | `/pqec/overhead` | `pqec overhead` | `{code:"surface", distance:5, target_error:1e-10}` |
| POST | `/pqec/sweep` | `pqec sweep` | `{code:"surface", t1_range:[10,200], points:20}` |
| GET | `/qspin/health` | — | — |
| POST | `/qspin/design` | `qspin design` | `{layout:"linear"\|"crossbar", qubits:4, platform:"sige"}` |
| POST | `/qspin/fidelity` | `qspin fidelity` | `{array:{DotArray}, gate:"exchange"}` |
| POST | `/qspin/stability` | `qspin stability` | `{plunger_range:[-1,1], barrier_range:[0,1]}` |
| POST | `/qspin/fab` | `qspin fab` | `{array:{DotArray}, platform:"sige"}` |
| POST | `/qspin/yield` | `qspin yield` | `{array:{DotArray}, platform:"sige", variation:5.0, samples:10000}` |
| GET | `/qion/health` | — | — |
| POST | `/qion/design` | `qion design` | `{type:"qccd", gate_zones:4, storage_zones:8, species:"ca40"}` |
| POST | `/qion/ms-gate` | `qion ms-gate` | `{species:"ca40", mode_freq:3.0}` |
| POST | `/qion/modes` | `qion modes` | `{ions:5, trap_freq:3.0, species:"ca40"}` |
| POST | `/qion/cooling` | `qion cooling` | `{qubit_species:"yb171", coolant_species:"be9"}` |
| POST | `/qion/schedule` | `qion schedule` | `{circuit:[[0,1]], trap:{TrapGeometry}}` |

### Bosonic / CoDesign / QAOA
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/bosonic/health` | — | — |
| POST | `/bosonic/simulate` | `rustybosonic simulate` | `{mode:"cat"\|"gkp"\|"binomial", alpha:2.0, n_fock:30}` |
| POST | `/bosonic/compare` | `rustybosonic compare` | `{modes:["cat","gkp"], alpha:2.0}` |
| POST | `/bosonic/optimize` | `rustybosonic optimize` | `{mode:"cat", target_fidelity:0.999}` |
| POST | `/bosonic/break-even` | `rustybosonic break-even` | `{mode:"cat", kappa_1:1.0, kappa_2:0.01}` |
| GET | `/codesign/health` | — | — |
| POST | `/codesign/optimize` | `rustycodesign optimize` | `{n_qubits:20, target_fidelity:0.9999}` |
| POST | `/codesign/roadmap` | `rustycodesign roadmap` | `{current:{}, target:{}}` |
| POST | `/codesign/compare-platforms` | `rustycodesign compare-platforms` | `{platforms:["sc","ion","neutral_atom"]}` |
| POST | `/codesign/what-if` | `rustycodesign what-if` | `{baseline:{}, changes:{t1:200.0}}` |
| POST | `/codesign/sensitivity` | `rustycodesign sensitivity` | `{design:{}, params:["t1","gate_fidelity"]}` |
| GET | `/qaoa/health` | — | — |
| POST | `/qaoa/maxcut` | `rustyqopt maxcut` | `{n_nodes:10, edge_density:0.5, p_layers:2}` |
| POST | `/qaoa/portfolio` | `rustyqopt portfolio` | `{n_assets:10, p_layers:2}` |
| POST | `/qaoa/tsp` | `rustyqopt tsp` | `{n_cities:8, p_layers:3}` |
| POST | `/qaoa/resources` | `rustyqopt resources` | `{problem:"maxcut", n_nodes:50}` |

### SWAP / OQFP / SCQ / Orchestrate
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/swap/health` | — | — |
| POST | `/swap/figure1d` | `rustyswap figure1d` | `{delta_mhz:0.0, t_max_ns:500}` |
| POST | `/swap/figure3a` | `rustyswap figure3a` | `{omega_mhz:5.0, t_max_ns:500}` |
| POST | `/swap/figure3c` | `rustyswap figure3c` | `{n_swaps:40}` |
| POST | `/swap/figure4c` | `rustyswap figure4c` | `{big_delta_mhz:25.0, n_delta:40, t_max_ns:2000}` |
| POST | `/swap/fock-convergence` | `rustyswap fock-convergence` | `{n_fock_max:6, omega_mhz:5.0}` |
| POST | `/swap/sw-validity` | `rustyswap sw-validity` | `{delta_min_ratio:1.0, delta_max_ratio:20.0, n_delta:30}` |
| POST | `/swap/nmodule-chain` | `rustyswap nmodule-chain` | `{routing:"sequential"\|"simultaneous", omega_12_mhz:5.0}` |
| POST | `/swap/tls-loss` | `rustyswap tls-loss` | `{alpha:0.01, beta:1.0, onset_mhz:5.0, n_omega:20}` |
| POST | `/swap/chi-sensitivity` | `rustyswap chi-sensitivity` | `{d_nominal_mm:0.764, sigma_d_mm:0.167, n_samples:10000}` |
| GET | `/oqfp/health` | — | — |
| POST | `/oqfp/validate` | `oqfp-cli validate` | `{spec:{OqfpChip}}` |
| POST | `/oqfp/summary` | `oqfp-cli summary` | `{spec:{OqfpChip}}` |
| POST | `/oqfp/diff` | `oqfp-cli diff` | `{old:{OqfpChip}, new:{OqfpChip}}` |
| POST | `/oqfp/create` | `oqfp-cli create` | `{template:"sc_9q"\|"sc_27q"\|"sc_127q"\|"spin_8q"\|"ion_32q"\|"atom_100q"}` |
| GET | `/scq/health` | — | — |
| POST | `/scq/spectrum` | `rustyscq spectrum` | `{circuit_type:"transmon"\|"fluxonium", ec:0.3, ej:15.0, n_evals:10}` |
| POST | `/scq/dispersion` | `rustyscq dispersion` | `{circuit_type:"transmon", ec:0.3, ej:15.0, g:0.1, n_fock:6}` |
| POST | `/scq/flux-sweep` | `rustyscq flux-sweep` | `{circuit_type:"fluxonium", ec:1.0, ej:5.0, el:0.5, points:50}` |
| POST | `/scq/coherence` | `rustyscq coherence` | `{circuit_type:"transmon", ec:0.3, ej:15.0, t_phi:100.0}` |
| GET | `/orchestrate/health` | — | — |
| GET | `/orchestrate/stages` | list templates | — |
| POST | `/orchestrate/validate` | `qorchestrate validate` | `{pipeline:{}}` |
| POST | `/orchestrate/run` | `qorchestrate run` | `{template:"full_calibration"\|"design_to_fab"\|..., params:{}}` |

### Mesh Generation (claw-mesh / claw-gds — Phase 7X)
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/mesh/health` | — | — |
| POST | `/mesh/transmon-cross` | claw-mesh inline | `{cross_length:300, cross_width:30, cross_gap:30, claw_length:80, claw_width:15, claw_gap:6}` |
| POST | `/mesh/rectangular-cavity-3d` | claw-mesh inline | `{cavity_length_um:10000, cavity_width_um:6000, cavity_height_um:3000}` |
| POST | `/mesh/tunable-transmon` | claw-mesh inline | `{squid_loop_width, squid_loop_height, ...}` |
| POST | `/mesh/xmon` | claw-mesh inline | `{arm_length, arm_width, gap, ...}` |
| POST | `/mesh/fluxonium` | claw-mesh inline | `{array_length, junction_spacing, ...}` |
| POST | `/mesh/cpw-resonator` | claw-mesh inline | `{length, center_width, gap_width, substrate_height}` |
| POST | `/mesh/chip` | claw-mesh inline | `{qubits:[{position,index,arm_length,arm_width}], couplers:[], chip_size:[w,h], mesh_size:200}` |
| POST | `/mesh/quality` | claw-mesh inline | `{mesh:{ClawTetMesh JSON}}` |
| GET | `/gds/health` | — | — |
| POST | `/gds/transmon-cross` | claw-gds inline | `{cross_length:300, ...}` — returns polygon/port JSON |
| POST | `/gds/rectangular-cavity-3d` | claw-gds inline | `{cavity_length_um, ...}` — returns polygon/port JSON |
| POST | `/gds/chip-layout` | claw-gds inline | `{cols:2, rows:2, pitch_x:1500, pitch_y:1500, qubit_params:{...}}` |
| POST | `/gds/export` | claw-gds inline | `{params:{TransmonCrossParams}}` — returns GDS-II bytes (hex) |

### ClawView Proxy (Phase 7Y — requires clawview on port 9090)
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/clawview/health` | clawview:9090 | — |
| GET | `/clawview/participation` | `/api/participation` | — |
| POST | `/clawview/streamlines` | `/api/streamlines` | `{seed_points:[], step_size:0.01}` |
| GET | `/clawview/isosurface` | `/api/isosurface` | — |
| GET | `/clawview/coupling` | `/api/coupling` | — |
| POST | `/clawview/surrogate/predict` | `/api/surrogate/predict` | `{gap:6.0, pad_width:15.0, claw_length:80.0, substrate_thickness:200.0}` |
| POST | `/clawview/cross-section` | `/api/cross-section` | `{plane:"xy", value:0.0}` |
| POST | `/clawview/layout/from-params` | `/api/layout/from_params` | `{params:{geometry}}` |
| GET | `/clawview/formats` | `/api/formats` | — |

### SymClaw
| Method | Path | Wraps | Key params |
|--------|------|-------|------------|
| GET | `/symclaw/health` | — | — |
| POST | `/symclaw/simplify` | `symclaw simplify` | `{expr:"sin(x)^2 + cos(x)^2"}` |
| POST | `/symclaw/differentiate` | `symclaw diff` | `{expr:"x^3", var:"x"}` |
| POST | `/symclaw/integrate` | `symclaw integrate` | `{expr:"x^2", var:"x", limits:[0,1]}` |
| POST | `/symclaw/solve` | `symclaw solve` | `{expr:"x^2 - 4", var:"x"}` |
| POST | `/symclaw/taylor` | `symclaw taylor` | `{expr:"exp(x)", var:"x", order:5, point:0}` |
| POST | `/symclaw/limit` | `symclaw limit` | `{expr:"sin(x)/x", var:"x", point:0}` |
| POST | `/symclaw/codegen` | `symclaw codegen` | `{expr:"x^2 + 2*x", lang:"python"\|"rust"\|"c"}` |
| POST | `/symclaw/linalg` | `symclaw linalg` | `{matrix:[[1,2],[3,4]], op:"det"\|"inv"\|"eig"}` |
| POST | `/symclaw/polynomial` | `symclaw polynomial` | `{expr:"x^2 - 5*x + 6", op:"factor"\|"roots"}` |
| POST | `/symclaw/analyze` | `symclaw analyze` | `{expr:"x^3 - x", var:"x"}` |

## bench/predict shorthand

For `/bench/predict` and `/bench/suggest`, you can pass individual scalar params (n_qubits, t1, t2, gate_fidelity, readout_fidelity) and the API builds a uniform ChipSpec internally. Or pass a full `chip_spec` object matching the ChipSpec schema.

## CORS

Permissive CORS enabled — safe for local Tailscale mesh access.

## Dependencies

All CLI binaries must be on PATH:
- `qtwin` ← rustyqtwin
- `freq` ← rustyfreq
- `xtalk` ← rustyxtalk
- `readout` ← rustyreadout
- `bench` ← rustybench-q
- `qstar` ← qstar-rs
- `surgery` ← rustysurgery
- `qexplore` ← rustyqexplore
- `rustypulse` ← rustypulse (pulse-cli)
- `rustystim` ← rustystim
- `rustybbq` ← rustybbq (bbq-cli)
- `rustyfloquet` ← rustyfloquet (floquet-cli)
- `rustypkg` ← rustypkg (pkg-cli)
- `rustycal` ← rustycal (cal-cli)
- `qfw` ← rustyqfw (rustyqfw-cli)
- `transpile` ← rustytranspile (rustytranspile-cli)
- `qspin` ← rustyqspin (rustyqspin-cli)
- `qion` ← rustyqion (rustyqion-cli)
- `qatom` ← rustyqatom (rustyqatom-cli)
- `pqec` ← rustypulse-qec (rustypulse-qec-cli)
- `rustyqml` ← rustyqml (qml-cli)
- `rustycryo` ← rustycryo (cryo-cli)
- `rustyqnet` ← rustyqnet (qnet-cli)
- `rustyqchem` ← rustyqchem (qchem-cli)
- `rustycryo-wiring` ← rustycryo-wiring (wiring-cli)
- `rustyextract` ← rustyextract (extract-cli)
- `rustybosonic` ← rustybosonic (bosonic-cli)
- `rustycodesign` ← rustycodesign (codesign-cli)
- `rustyqopt` ← rustyqopt (qaoa-cli)
- `rustyswap` ← rustyswap
- `oqfp-cli` ← oqfp-cli
- `rustyscq` ← rustyscq (scq-cli)
- `qorchestrate` ← rustyqorchestrate (service on port 8767)
- `symclaw` ← symclaw (symclaw-cli)
- **claw-mesh / claw-gds** ← linked directly as Rust path deps (no binary needed)
- **clawview** ← `claw-view` binary, runs on port 9090 (`/nvme/quantum/clawview`)

Install Tier-1 tools (from rustyqtwin/rustyfreq/etc workspaces):
```bash
sudo cp target/release/{qtwin,freq,xtalk,readout,bench,qstar,surgery,qexplore} /usr/local/bin/
```

Install Tier-2 tools (from their respective workspaces):
```bash
sudo cp /nvme/quantum/rustybbq/target/release/rustybbq /usr/local/bin/
sudo cp /nvme/quantum/rustyfloquet/target/release/rustyfloquet /usr/local/bin/
sudo cp /nvme/quantum/rustypkg/target/release/rustypkg /usr/local/bin/
sudo cp /nvme/quantum/rustycal/target/release/rustycal /usr/local/bin/
sudo cp /nvme/quantum/rustyqfw/target/release/qfw /usr/local/bin/
sudo cp /nvme/quantum/rustytranspile/target/release/transpile /usr/local/bin/
# rustypulse and rustystim are installed via their own deploy scripts
```
