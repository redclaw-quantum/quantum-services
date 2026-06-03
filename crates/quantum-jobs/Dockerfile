# This per-crate Dockerfile is intentionally a pointer.
#
# After the §4.8 consolidation, building from this dir doesn't work — the
# workspace Cargo.lock lives at the parent (quantum-services/) and this
# crate's Cargo.toml is a workspace member, not a standalone Cargo project.
#
# The real build uses the workspace-root Dockerfile:
#
#   cd ../..   # to quantum-services/
#   docker build -t quantum-jobs:latest --target quantum-jobs .
#
# That root Dockerfile builds quantum-api and quantum-jobs in a single
# cached cargo invocation. See quantum-services/Dockerfile for details.
