## IQ-8: Block store — in-memory in phase-1, redb-backed at S8.5

**Status:** Accepted
**Date:** 2026-05-08
**Sprint context:** S8 (DAG store + recovery)

### Question

S8 calls for a durable block store that survives node restart. The
in-memory store is enough to property-test recovery semantics
(replay-from-store reaches the same state as live execution); the
durable redb-backed store is what makes recovery real in deployment.

### Decision

Phase-1 ships `InMemoryBlockStore`. Redb-backed durable storage lands
in **S8.5**, before any network deployment. The trait surface
(`BlockStore::{put, get_by_hash, get_by_height, latest, len, iter_from}`)
is identical between impls.

This mirrors **IQ-1** (state store: redb-now, RocksDB-prod): the
recovery property test exercises the trait, not the impl, so the swap
to redb is invisible to test code.

### Why phase-1 ships in-memory

- The recovery exit gate (`recover_matches_live_state` at 10k cases)
  exercises the replay machinery, not durability.
- Redb integration adds disk-IO, tempdir lifetimes, and per-test
  setup cost the property tests don't need.
- The S8.5 swap is a single new module behind the same trait.

### What S8.5 will add

- `RedbBlockStore::open(path)` with two tables: `blocks_by_hash`,
  `height_to_hash`.
- Atomic write transactions per `put` (block + height index in one
  txn).
- Iteration via `redb::ReadableTable::range`.
- Property-test parity: same `recover_matches_live_state` runs against
  both backends.

### Consequences

- **Code:** `gsxdb-bridge::recovery::store` ships `BlockStore` trait +
  `InMemoryBlockStore` impl. `RedbBlockStore` is S8.5.
- **Tests:** Recovery property test runs against `InMemoryBlockStore`.
- **Spec:** `docs/spec/recovery.md` documents the swap point.

### What this leaves open

- DAG / multi-parent block representation. Phase-1 is linear (single
  parent). The `Block` struct can grow `parents: Vec<BlockHash>` with
  no commitment-scheme change; out of scope for phase-1.
- Snapshot checkpoints. Replay from genesis is fine for short chains;
  production wants periodic state snapshots so recovery isn't O(chain
  length). Probably IQ-9.

### Propagation checklist

- [x] Code: trait + in-memory impl
- [x] Tests: 10k cases of `recover_matches_live_state`
- [ ] S8.5: `RedbBlockStore` impl
- [ ] IQ-9: snapshot checkpoints
- [ ] DAG variant: `parents: Vec<BlockHash>` if needed
