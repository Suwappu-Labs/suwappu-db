# gsxdb-state

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**Authoritative state lane.** Owns balances, the Verkle-shaped state
tree, dual-VM projectors, snapshots, the DAG store, and metrics.

> Downstream code should depend on **[`gsxdb-types`](../gsxdb-types)**,
> not on this crate directly. `gsxdb-state` is internal — its public
> APIs may evolve in any pre-1.0 minor bump.

## What it owns

- **Canonical balance map** (`State`, `BalanceSlot`, `BalanceStore`)
  with pluggable in-memory or redb backend.
- **Dual-VM projection invariant** (Proposition 1) — `EVM
  balanceOf == Move Coin.value` enforced by the tree's
  serialization layer + a 10k-case proptest.
- **State tree** — 256-ary trie with BLAKE3 (default) or
  banderwagon + IPA Verkle commitments (`production-verkle`
  feature). Inclusion / absence proofs.
- **Snapshot** — `StateSnapshot::from_state` →
  `restore_into_state` round-trip, sorted-encode for byte-
  idempotent capture.
- **DAG store** — multi-parent block linkage with children index +
  ancestors/descendants/tips queries.
- **Metrics** — Gauge / Counter / Histogram with Prometheus
  exposition format output.

## Lane separation invariant

`gsxdb-state` is the *only* crate that owns canonical state, and
the *only* writer is `gsxdb-bridge` (guarded by the `BridgeToken`
capability). `gsxdb-lane` (ingest) cannot import `gsxdb-state`. This
is enforced at the type level by `BridgeToken`'s `pub(crate)`
constructor and at build time by `scripts/check-lane-separation.sh`.

## Feature flags

| Flag | Pulls in | Default |
|---|---|---|
| `production-move-executor` | Aptos `move-vm-runtime` + `move-bytecode-verifier` (~100 transitive crates from `aptos-core` git pin) | off |
| `production-verkle` | `banderwagon` + `ipa-multipoint` from `crate-crypto/rust-verkle` | off |

Defaults compile the phase-1 substrate (BLAKE3 commitments + mock
Move executor).

## Tests

```sh
cargo test -p gsxdb-state                       # 128 unit/integration
cargo test -p gsxdb-state --features production-verkle      # +Verkle paths
PROPTEST_CASES=10000 cargo test -p gsxdb-state --release   # exit-gate
```

## Specs

- [`docs/spec/pbm-balance-slot.md`](../../docs/spec/pbm-balance-slot.md)
- [`docs/spec/dual-vm-projectors.md`](../../docs/spec/dual-vm-projectors.md)
- [`docs/spec/verkle-state-tree.md`](../../docs/spec/verkle-state-tree.md)
- [`docs/spec/lane-separation.md`](../../docs/spec/lane-separation.md)
- [`docs/spec/move-vm-session-layer.md`](../../docs/spec/move-vm-session-layer.md)
- [`docs/spec/observability.md`](../../docs/spec/observability.md)
