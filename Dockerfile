# Consolidated Dockerfile for the quantum-services workspace.
#
# Replaces the pre-§4.8-consolidation per-crate Dockerfiles in
# crates/quantum-api/ and crates/quantum-jobs/ which were broken:
#   (1) they COPY'd Cargo.toml from the crate dir but Cargo.lock lives
#       at the workspace root post-consolidation;
#   (2) they pinned rust:1.87-slim but quantum-api/Cargo.toml's
#       rust-version is 1.95;
#   (3) they had no .dockerignore so target/ would land in the build
#       context;
#   (4) and most importantly: quantum-api references claw-mesh / claw-gds /
#       claw-tet via `../../../quantum-mesh/crates/...` path-deps, which
#       escape the quantum-services/ build context entirely.
#
# Build from the **projects root** so both quantum-services AND quantum-mesh
# are in the build context:
#
#   cd /home/osobh/projects/
#   docker build -f quantum-services/Dockerfile -t quantum-api:latest --target quantum-api .
#   docker build -f quantum-services/Dockerfile -t quantum-jobs:latest --target quantum-jobs .
#
# Both builds share the `builder` stage's cargo cache. With BuildKit (default
# on Docker ≥18.09) the dependency-compile pass is reused between the two
# `--target` builds in the same session.

# ----------------------------------------------------------------------------
# Stage: builder — compile both service binaries in one cargo invocation.
# ----------------------------------------------------------------------------
FROM rust:1.95-slim AS builder

# pkg-config + libssl-dev are required by reqwest (transitively, native-tls).
# build-essential pulls in cc/ld for any C deps inside transitive crates.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev build-essential \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Bring in BOTH workspaces. quantum-api's external path-deps point into
# quantum-mesh; layout under /build mirrors the host's /home/osobh/projects/
# so those relative paths resolve.
COPY quantum-services ./quantum-services
COPY quantum-mesh     ./quantum-mesh

WORKDIR /build/quantum-services
RUN cargo build --release --bin quantum-api --bin quantum-jobs

# ----------------------------------------------------------------------------
# Stage: runtime-base — shared runtime image.
#
# `trixie-slim` matches the GLIBC the rust:1.95-slim builder links against
# (≥ 2.39). bookworm-slim's GLIBC 2.36 is too old — the binary fails to
# start with "version GLIBC_2.39 not found".
# ----------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime-base
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# ----------------------------------------------------------------------------
# Stage: quantum-api (port 8765 — REST gateway for Tier 1 CLI tools).
# ----------------------------------------------------------------------------
FROM runtime-base AS quantum-api
COPY --from=builder /build/quantum-services/target/release/quantum-api /usr/local/bin/quantum-api
ENV QUANTUM_API_PORT=8765 \
    RUST_LOG=quantum_api=info,tower_http=warn
EXPOSE 8765
CMD ["/usr/local/bin/quantum-api"]

# ----------------------------------------------------------------------------
# Stage: quantum-jobs (port 8766 — async job queue + sidecar proxies).
# ----------------------------------------------------------------------------
FROM runtime-base AS quantum-jobs
COPY --from=builder /build/quantum-services/target/release/quantum-jobs /usr/local/bin/quantum-jobs
ENV QUANTUM_JOBS_PORT=8766 \
    QEM_URL=http://qem:8430 \
    QPUDIDP_URL=http://qpu-didp:8420 \
    RUST_LOG=quantum_jobs=info,tower_http=warn
EXPOSE 8766
CMD ["/usr/local/bin/quantum-jobs"]
