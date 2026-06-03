# rustyqorchestrate

Unified orchestration engine for the QuantumRedClaw platform. Chains quantum design tools into typed, resumable, observable pipelines — from inverse device design through frequency planning, pulse optimization, and QEC assessment.

**Port:** `8767`
**Binary:** `qorchestrate`
**Systemd:** `qorchestrate.service`

---

## Quick Start

```bash
# Build
cargo build --release

# Serve HTTP API + drift monitor
qorchestrate serve --port 8767

# Run a built-in template
qorchestrate run --template design-to-chip \
  --param qubit_frequency_ghz=5.0 \
  --param anharmonicity_mhz=-250

# Show the pipeline DAG
qorchestrate dag --template design-to-chip

# Validate a custom pipeline TOML
qorchestrate validate --file my_pipeline.toml
```

---

## Pipeline Templates

Templates live in `templates/` and are loaded at startup.

| Template | Stages | Purpose |
|----------|--------|---------|
| `design-to-chip` | 11 | Hamiltonian targets → OQFP chip spec (QPUDIDP → qem → freq → xtalk → readout → pulse → QEC → bench → twin → OQFP) |
| `chip-to-calibration` | 5 | Measured hardware drift → recalibration (replaces `recal_trigger.sh`) |
| `yield-sweep` | 5 | Monte Carlo manufacturing yield over fabrication parameter space |
| `full-loop` | 4 | Full design lifecycle with sub-pipeline composition |
| `active-design-loop` | 1 | Bayesian GP outer optimization loop |

---

## HTTP API (port 8767)

| Method | Path | Description |
|--------|------|-------------|
| POST | `/pipeline/run` | Submit pipeline run |
| GET | `/pipeline/{id}/status` | Poll run status |
| GET | `/pipeline/{id}/stream` | SSE real-time event stream |
| GET | `/pipeline/{id}/result` | Final result JSON |
| GET | `/pipeline/{id}/artifacts` | List artifact keys |
| POST | `/pipeline/validate` | Validate pipeline template TOML |
| POST | `/pipeline/resume` | Resume from checkpoint |
| DELETE | `/pipeline/{id}` | Cancel a run |
| GET | `/pipeline/templates` | List all available templates |
| GET | `/pipeline/templates/{name}` | Get template TOML |
| GET | `/pipeline/templates/{name}/dag` | Mermaid DAG diagram |

Also accessible via quantum-api proxy at `/orchestrate/*`.

---

## Stage Types

All 30+ stage types map to quantum-api endpoints via HTTP. Key groups:

| Group | Stage Types |
|-------|-------------|
| QPUDIDP | `QpudidpRmflow`, `QpudidpCmaes` |
| Physics | `QemSolve`, `ScqSimulate` |
| Chip | `FreqOptimize`, `XtalkAnalyze`, `ReadoutDesign` |
| Pulse | `GrapeOptimize`, `DragOptimize`, `FloquetGrape` |
| QEC | `PqecAssess`, `QecThreshold`, `SurgeryResources` |
| Parametric | `QcircQuantize`, `QcircProcesses`, `QcircPumpDesign`, `QcircFloquet`, `QcircRegimeScan`, `QcircConstraints`, `QcircSummary` |
| Explore | `QexplorePareto`, `FreqYield` |
| Bench | `BenchPredict` |
| Twin | `TwinCompare`, `TwinRecalibrate`, `TwinQecUpdate`, `TwinMock` |
| OQFP | `OqfpBuild`, `OqfpValidate` |
| Meta | `Skip`, `PipelineCall`, `Batch`, `BayesianOuterLoop` |

---

## Crates

| Crate | Role |
|-------|------|
| `rustyqorchestrate-core` | Stage trait, `StageContext`, `StageError`, `StageType` enum, pipeline DAG |
| `rustyqorchestrate-executor` | `PipelineExecutor`, `StageRegistry`, checkpoint/resume, SSE streaming |
| `rustyqorchestrate-stages` | All stage implementations — each calls a quantum-api HTTP endpoint |
| `rustyqorchestrate-cli` | `qorchestrate` binary: `run`, `serve`, `dag`, `validate`, `status` subcommands; XEB verify |

---

## Drift Monitor

When running in `serve` mode, an embedded monitor polls `POST /qtwin/compare` every 600 s. If `critical_count > 0` it auto-submits a `chip-to-calibration` pipeline run. This replaces the `recal_trigger.sh` cron script.

---

## Parametric Process Pipelines

The `QcircQuantize` → `QcircProcesses` → `QcircPumpDesign` → `QcircFloquet` → `QcircRegimeScan` → `QcircConstraints` → `QcircSummary` stage chain orchestrates full SNAIL/JRM parametric amplifier design. Use the `qcirc_pipeline` endpoint on quantum-api for the single-shot version.

---

## Part of QuantumRedClaw

This repo is part of the [QuantumRedClaw](https://github.com/quantumredclaw) quantum processor design platform.

## License

LicenseRef-RedClaw-Proprietary
