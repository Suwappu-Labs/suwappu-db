# suwappudb-lane

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**Transaction ingest lane.** Accepts EVM- or Move-shaped
transactions and forwards them to `suwappudb-bridge` for validation +
execution.

> Downstream code should depend on **[`suwappudb-types`](../suwappudb-types)**,
> not on this crate directly. `suwappudb-lane` is internal — its public
> APIs may evolve in any pre-1.0 minor bump.

## What it owns

- Inbound transaction queueing and basic envelope checks.
- The "ingest" side of the lane-separation invariant — `suwappudb-lane`
  cannot import `suwappudb-state` directly; all mutations go through
  `suwappudb-bridge::Bridge`.

## Lane separation invariant

By repo convention enforced at compile time and via
`scripts/check-lane-separation.sh`, `suwappudb-lane` has zero
dependency on `suwappudb-state`. The only path from ingest to canonical
state is through `suwappudb-bridge::Bridge::submit`, which mints the
capability-typed `BridgeToken` after validating the intent.

See [`docs/spec/lane-separation.md`](../../docs/spec/lane-separation.md)
for the full invariant.

## Tests

```sh
cargo test -p suwappudb-lane
```

## Status

The lane crate is intentionally thin in v0.1.0-pre — most ingest
shape lives upstream in the consensus layer (`suwappu-dag`). The crate
exists to anchor the lane-separation type-system gate; richer
ingest semantics (rate limiting, mempool, etc.) arrive when the
phase-1 substrate wires into a live consensus path.
