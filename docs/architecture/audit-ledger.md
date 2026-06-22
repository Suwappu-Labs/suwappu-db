# Placeholder and deferred-fact audit ledger

Line-level ledger tracking placeholder/deferred facts and their execution owner.

| ID | File:Line | Placeholder text (summary) | Classification | Required action | Owner sprint |
|---|---|---|---|---|---|
| A-001 | `crates/suwappudb-lane/src/lib.rs:32` | "Phase-1 placeholder" mempool wording | intentional-defer | Replace FIFO mempool with S5+/prod queue semantics | S9 |
| A-002 | `crates/suwappudb-bridge/src/recovery/replay.rs:79-80` | incremental parent source described as placeholder | intentional-defer | Define/implement checkpoint start contract for non-zero replay | S12 (IQ-9) |
| A-003 | `crates/suwappudb-state/src/tree/ops.rs:127` | commitment placeholder branch | resolved | Removed placeholder state; verification now uses `Option<Commitment>` path directly | S8.5 (done) |
| A-004 | `crates/suwappudb-state/src/tree/commit.rs:37` | const placeholder commitment | intentional-defer | Replace with real commitment backend under Verkle swap | S10 |
| A-005 | `docs/spec/README.md:16-17` | TODO markers for missing specs | resolved | Added S1/S2 spec docs and updated spec status table to current written set | S8.5 (done) |
| A-006 | `docs/architecture/overview.md:110` | runtime row had `TBD` | resolved | Replaced `TBD` with explicit S9 decision gate and target adapter wording | S8.5 (done) |
| A-007 | `docs/architecture/dual-projection.md:118-122` | placeholder IQ references for nonce/address | intentional-defer | Close IQ-4/IQ-5 with concrete semantics + code changes | S9 |
| A-008 | `docs/spec/recovery.md:188-194` | DAG/snapshot marked open | intentional-defer | Implement checkpoints + DAG replay order policy | S12 |
| A-009 | `crates/suwappudb-bridge/src/vm/mod.rs:9` | mock executor caveat | intentional-defer | Add real Move VM adapter and migration harness | S9 |
| A-010 | `crates/suwappudb-bridge/src/anchor/mod.rs:14` | on-chain registry/signatures deferred | intentional-defer | Implement Solidity registry + signature parity matrix | S11 |

## Triage status

- **P0 (correctness/security)**: A-003
- **P1 (behavior/spec mismatch)**: A-005, A-006
- **P2 (planned defer/docs)**: A-001, A-002, A-004, A-007, A-008, A-009, A-010

## Execution notes

- This file is intentionally append-only by row ID.
- Any row moved to `stale` requires a linked fix PR before sprint close.
- New placeholder discoveries must be added in the same PR that finds them.
