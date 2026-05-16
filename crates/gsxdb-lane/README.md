# gsxdb-lane

[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](../../LICENSE)

**Transaction ingest lane.** Accepts EVM- or Move-shaped
transactions and forwards them to `gsxdb-bridge` for validation +
execution.

> Downstream code should depend on **[`gsxdb-types`](../gsxdb-types)**,
> not on this crate directly. `gsxdb-lane` is internal — its public
> APIs may evolve in any pre-1.0 minor bump.

## What it owns

- Inbound transaction queueing and basic envelope checks.
- The "ingest" side of the lane-separation invariant — `gsxdb-lane`
  cannot import `gsxdb-state` directly; all mutations go through
  `gsxdb-bridge::Bridge`.

## Lane separation invariant

By repo convention enforced at compile time and via
`scripts/check-lane-separation.sh`, `gsxdb-lane` has zero
dependency on `gsxdb-state`. The only path from ingest to canonical
state is through `gsxdb-bridge::Bridge::submit`, which mints the
capability-typed `BridgeToken` after validating the intent.

See [`docs/spec/lane-separation.md`](../../docs/spec/lane-separation.md)
for the full invariant.

## Tests

```sh
cargo test -p gsxdb-lane
```

## Status

The lane crate is intentionally thin in v0.1.0-pre — most ingest
shape lives upstream in the consensus layer (`gsx-dag`). The crate
exists to anchor the lane-separation type-system gate; richer
ingest semantics (rate limiting, mempool, etc.) arrive when the
phase-1 substrate wires into a live consensus path.
