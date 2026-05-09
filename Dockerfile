# Multi-stage build for gsxdb-server
# Stage 1: Builder
FROM rust:1.78 AS builder

WORKDIR /build

# Copy workspace files
COPY Cargo.lock Cargo.toml ./
COPY crates ./crates

# Build gsxdb-server in release mode
RUN cargo build --release -p gsxdb-server

# Stage 2: Runtime
FROM debian:bookworm-slim

# Install ca-certificates for HTTPS
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

# Create data directory
RUN mkdir -p /data/gsxdb

# Copy binary from builder
COPY --from=builder /build/target/release/gsxdb-server /usr/local/bin/gsxdb-server

# Create app directory
WORKDIR /app

# Default configuration file location
ENV CONFIG_PATH=/etc/gsxdb/config.toml

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD curl -f http://localhost:8660/health || exit 1

# Expose RPC and metrics ports
EXPOSE 8660 9660

# Run the server
ENTRYPOINT ["gsxdb-server"]
