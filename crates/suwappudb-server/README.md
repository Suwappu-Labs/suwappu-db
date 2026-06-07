# suwappudb-server

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**HTTP server binary.** Exposes JSON-RPC for state queries +
parity checks, `/health` for liveness, `/metrics` for Prometheus
scraping.

This is a binary, not a library — depend on it via `docker pull` or
the released cargo binary (see
[INTEGRATORS.md "Distribution — Docker"](../../INTEGRATORS.md#distribution--docker-c8)),
not via `cargo add`.

## Endpoints

| Method | Path | Auth | Notes |
|---|---|---|---|
| `GET` | `/health` | none | Liveness probe, returns `{"status":"ok"}` |
| `GET` | `/metrics` | none | Prometheus exposition format (summary quantiles) |
| `POST` | `/v1/rpc` | optional bearer | Canonical JSON-RPC 2.0 endpoint |
| `POST` | `/rpc` | optional bearer | Deprecated alias for `/v1/rpc`; warn-on-hit |

JSON-RPC method catalogue in [`docs/api/rpc-schema.json`](../../docs/api/rpc-schema.json).

## Auth posture

- `/health` and `/metrics` are always unauthenticated (liveness
  probes need them).
- `/v1/rpc` is **optionally** gated behind
  `Authorization: Bearer <token>` when `SUWAPPUDB_BEARER_TOKEN` is set
  in the environment. Constant-time token compare.
- Bearer token is a **defence-in-depth** layer, not a primary
  control. Production deployments must front this service with a
  firewall / VPC SG / nginx / Cloudflare Access. See
  [`docs/architecture/deployment-topology.md`](../../docs/architecture/deployment-topology.md)
  "RPC endpoint auth posture".

## Run

```sh
cargo run --release --bin suwappudb-server
# or
docker run --rm -p 8660:8660 \
    ghcr.io/globalsettlementnetwork/suwappu-db:v0.1.0-pre
```

Hardened production run:

```sh
docker run --rm \
    -p 127.0.0.1:8660:8660 \
    -e SUWAPPUDB_BEARER_TOKEN=$(openssl rand -hex 32) \
    -e RUST_LOG=info \
    ghcr.io/globalsettlementnetwork/suwappu-db:v0.1.0-pre
```

## Configuration

| Env | Default | What it does |
|---|---|---|
| `SUWAPPUDB_BEARER_TOKEN` | (unset) | Enables bearer-token auth on `/v1/rpc` |
| `RUST_LOG` | (unset) | Standard tracing-subscriber filter |
| `CONFIG_PATH` | `/etc/suwappudb/config.toml` | Reserved for future TOML config file |

Default port is `8660`. The container exposes both `8660` (RPC + metrics) and `9660` (reserved side-car metrics port; currently shares 8660).

## Tests

```sh
cargo test -p suwappudb-server
```

9 tests covering the bearer-auth middleware + JSON-RPC handler
shapes.
