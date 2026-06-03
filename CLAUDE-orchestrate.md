# rustyqorchestrate

Unified orchestration engine for QuantumRedClaw. Chains all quantum design
tools (QPUDIDP → qem-rs → rustyfreq → rustyxtalk → rustypulse → qstar-rs)
into typed, resumable, observable pipelines.

## Port

8767 (standalone) or integrated into quantum-api at `/pipeline/*`

## Run

```bash
# Serve HTTP API + monitor daemon
qorchestrate serve --port 8767

# Run a pipeline inline
qorchestrate run --template design-to-chip \
  --param qubit_frequency_ghz=5.0 \
  --param anharmonicity_mhz=-250

# Get pipeline status
qorchestrate status --run-id <uuid>

# Show DAG
qorchestrate dag --template design-to-chip
```

## Templates (in /nvme/quantum/rustyqorchestrate/templates/)

| Template | Stages | Purpose |
|----------|--------|---------|
| design-to-chip | 11 | Hamiltonian targets → OQFP chip spec |
| chip-to-calibration | 5 | Measured hardware → recalibration (replaces recal_trigger.sh) |
| yield-sweep | 5 | Monte Carlo yield over fabrication parameter space |
| full-loop | 4 | Full design lifecycle with sub-pipeline composition |
| active-design-loop | 1 | Bayesian GP outer optimization |

## HTTP API (port 8767)

| Method | Path | Description |
|--------|------|-------------|
| POST | /pipeline/run | Submit pipeline |
| GET | /pipeline/{id}/status | Poll status |
| GET | /pipeline/{id}/stream | SSE event stream |
| GET | /pipeline/{id}/result | Final result |
| GET | /pipeline/{id}/artifacts | Artifact keys |
| POST | /pipeline/validate | Validate template |
| POST | /pipeline/resume | Resume from checkpoint |
| DELETE | /pipeline/{id} | Cancel |
| GET | /pipeline/templates | List templates |
| GET | /pipeline/templates/{name} | Get template TOML |
| GET | /pipeline/templates/{name}/dag | Mermaid DAG |

## Monitor daemon

Embedded in `serve`. Polls `POST /qtwin/compare` for chip drift every 600s.
Auto-submits `chip-to-calibration` pipeline when `critical_count > 0`.
Replaces `recal_trigger.sh`.

## Crate structure

- `rustyqorchestrate-core` — pipeline DSL, Stage trait, DAG builder, typed I/O structs
- `rustyqorchestrate-executor` — async executor, checkpoint, SSE, stage registry
- `rustyqorchestrate-stages` — 25 concrete stage implementations
- `rustyqorchestrate-cli` — qorchestrate binary, Axum HTTP server, monitor daemon

## Data flow (design-to-chip)

```
HamiltonianTarget → qpudidp_rmflow → GeometryCandidates
  → scq_simulate → DevicePhysics
    → freq_optimize ─┐
    → xtalk_analyze  ├─ grape_optimize → OptimizedPulse
    → readout_design ─┘   → pqec_assess → QecAssessment
                              → bench_predict → QvClops
                                → oqfp_build → OqfpSpec (8 layers)
                                  → oqfp_validate → PIPELINE OUTPUT
```

## ZeroClaw actions

pipeline_run, pipeline_status, pipeline_result, pipeline_list_templates, pipeline_dag
