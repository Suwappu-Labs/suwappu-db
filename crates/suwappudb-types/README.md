# suwappudb-types

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**Frozen public-type surface for suwappu-db.** This is the crate
downstream wallets, indexers, parallel verifiers, and chain-listeners
depend on.

`suwappudb-types` is a thin re-export facade over `suwappudb-state` and
`suwappudb-bridge`. It carries its own [`version`](./Cargo.toml) so the
frozen surface can be bumped independently of the internal crates.

## Use this if

- You're writing a downstream Rust client (wallet / indexer /
  parallel verifier) that needs the `Anchor` struct, the
  `AuthScheme` discriminants, or the EIP-191 verifier surface.
- You want a stable target on your `Cargo.toml` instead of pinning
  the internal crates whose APIs may evolve.

## Use the internal crates directly if

- You need internal traits (`MoveExecutor`, `CommitmentScheme`,
  `BlockStore`) that are intentionally NOT re-exported.
- You're modifying suwappu-db itself.

## Add to your project

```toml
[dependencies]
suwappudb-types = { git = "https://github.com/Suwappu-Labs/suwappu-db", tag = "v0.1.0-pre" }
```

Pin to a tag — pre-1.0, minor bumps may include breaking changes
per [INTEGRATORS.md "Stability promises"](../../INTEGRATORS.md#stability-promises).

## What's exported

- **State + addresses** (`Address`, `Balance`, `Commitment`, `State`,
  `StateChange`, `BridgeToken`, dual-projection nonce/coin-value
  types).
- **State tree** (`tree::`) — `StateTree`, `Node`, `Proof`,
  `ProofStep`, `Blake3Scheme`, `CommitmentScheme`.
- **Snapshot** (`snapshot::`) — `StateSnapshot`, `SnapshotManager`.
- **DAG** (`dag::`) — `DagBlock`, `DagStore`, `BlockHash`.
- **Metrics** (`metrics::`) — `Metrics`, `Gauge`, `Histogram`,
  `Counter`, `Timer`.
- **Anchor / verifier** — `Anchor`, `AnchorHash`, `AnchorLog`,
  `AnchorDispatcher`, `ParityResult`, `AuthScheme`, `EthAddress`,
  `AnchorAuthCredential`, `ExpectedVerifier`, `CredentialVerifyError`,
  `EcdsaVerifyError`, `verify_credential`, `verify_ecdsa`,
  `eth_signed_message_hash`, `Sp1PublicValues`, `ECDSA_SIG_LEN`.

## Stability contract

- `#[non_exhaustive]` enum-variant / struct-field additions = **non-
  breaking** (don't write exhaustive matches on these).
- Field removal / rename / type-change = **major bump**.
- Internal traits are not re-exported here; they live in their
  internal crates and may break in any minor bump pre-1.0.

## Tests

```sh
cargo test -p suwappudb-types
```

One smoke test (`frozen_surface_smoke`) constructs each major shape
— catches re-export breakage at compile time.

## Related

- [INTEGRATORS.md](../../INTEGRATORS.md) — full integrator guide.
- [CHANGELOG.md](../../CHANGELOG.md) — per-release deltas.
