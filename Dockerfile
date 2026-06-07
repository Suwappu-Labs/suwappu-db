# Multi-stage build for suwappudb-server.
#
# C8: Rust toolchain bumped 1.78 → 1.88 to match the workspace
# `rust-version` in Cargo.toml. The C2 release workflow builds the
# cargo binary directly; this Dockerfile is the container path.
#
# Layer ordering: deps-only build before COPY src so dependency
# compilation caches across source changes. The COPY crates step
# invalidates the source layer when any crate changes; the deps
# layer below stays warm.

# Stage 1: Builder
FROM rust:1.88 AS builder

WORKDIR /build

# Copy workspace files
COPY Cargo.lock Cargo.toml ./
COPY crates ./crates

# Build suwappudb-server in release mode
RUN cargo build --release -p suwappudb-server

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install ca-certificates (HTTPS to op-reth + L1 RPCs) + curl
# (HEALTHCHECK probe). `apt-get install --no-install-recommends`
# keeps the image small.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Create data directory
RUN mkdir -p /data/suwappudb

# Copy binary from builder
COPY --from=builder /build/target/release/suwappudb-server /usr/local/bin/suwappudb-server

# Create app directory
WORKDIR /app

# Default configuration file location
ENV CONFIG_PATH=/etc/suwappudb/config.toml

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8660/health || exit 1

# Expose RPC and metrics ports
EXPOSE 8660 9660

# Run the server
ENTRYPOINT ["suwappudb-server"]
