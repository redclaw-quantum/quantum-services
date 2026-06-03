# CLAUDE.md — quantum-jobs

Async job queue for Tier 2 long-running quantum tools, plus HTTP proxies to qem (port 8430) and qpu-didp (port 8420) which are already HTTP servers.

## Ports

| Service | Port | Binary |
|---------|------|--------|
| quantum-jobs | 8766 | `/usr/local/bin/quantum-jobs` |
| qem (FEM solver) | 8430 | `/usr/local/bin/qem` |
| qpu-didp (inverse design) | 8420 | `/usr/local/bin/qpu-didp` |

## Start All Services

```bash
sudo systemctl start qem
sudo systemctl start qpu-didp
sudo systemctl start quantum-jobs
```

## API Reference

### Health
```
GET /health
```
Returns status of both sidecar servers (qem, qpu-didp) plus job queue stats.

### Submit Pulse Job
```
POST /jobs/pulse
{
  "subcommand": "optimize" | "simulate" | "drag" | "readout",
  "params": {
    "gate": "X",
    "grape": true,
    "steps": 50,
    "iterations": 500
  }
}
→ {"job_id": "uuid", "status": "queued", "tool": "pulse"}
```

### Submit Stim Job
```
POST /jobs/stim
{
  "subcommand": "gen" | "sample" | "detect" | "analyze-errors",
  "circuit": "QUBIT_COORDS...",   // optional, for sample/detect
  "params": {
    "code": "surface_code",       // for gen: repetition_code | surface_code | color_code
    "distance": 3,
    "rounds": 5,
    "noise": 0.001
  }
}
```

### Poll Job Status
```
GET /jobs/{id}
→ {id, tool, status:"queued"|"running"|"done"|"failed"|"cancelled", result, error, ...}
```

### Get Result
```
GET /jobs/{id}/result
→ result JSON when done
```

### Cancel Job
```
DELETE /jobs/{id}
```

### List All Jobs
```
GET /jobs
```

### Proxy to QEM
```
POST /proxy/qem/solve_unified    → http://localhost:8430/solve_unified
POST /proxy/qem/solve_lom        → http://localhost:8430/solve_lom
POST /proxy/qem/solve_lom_fem    → http://localhost:8430/solve_lom_fem
POST /proxy/qem/chip             → http://localhost:8430/chip
```

### Proxy to QPU-DIDP
```
POST /proxy/qpudidp/tools/surrogate_predict
POST /proxy/qpudidp/tools/inverse_design
POST /proxy/qpudidp/tools/physics_validate
POST /proxy/qpudidp/tools/codegen_qiskit
POST /proxy/qpudidp/tools/inverse_design_flow
POST /proxy/qpudidp/tools/circuit_select
POST /proxy/qpudidp/tools/oracle_evaluate
```

## Environment Variables

| Variable | Default | Purpose |
|----------|---------|---------|
| `QUANTUM_JOBS_PORT` | 8766 | Port for quantum-jobs service |
| `QEM_URL` | http://127.0.0.1:8430 | QEM server URL |
| `QPUDIDP_URL` | http://127.0.0.1:8420 | QPU-DIDP server URL |
| `QEM_PORT` | 8430 | qem server port |
| `QEM_SERVER_URL` | — | URL of QEM server (for qpu-didp) |

## Full Service Stack

```
ZeroClaw (port 9090)
    │
    ├── quantum-api (port 8765) ← Tier 1 stateless tools
    │       qtwin, freq, xtalk, readout, bench, qstar, surgery, qexplore
    │
    └── quantum-jobs (port 8766) ← Tier 2 async + sidecar proxy
            ├── rustypulse (CLI async jobs)
            ├── rustystim (CLI async jobs)
            ├── qem (port 8430) — FEM eigenmode solver
            └── qpu-didp (port 8420) — inverse design agent
```
