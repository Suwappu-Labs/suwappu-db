# Sprint S7 + S8 state — phase-1 close

**Branch:** `phase1/anchor-and-recovery`
**Started:** 2026-05-08
**Exit gates:**
- S7: `cross_chain_parity_holds` — anchors at the same height across N chains agree on state root, tampering is detected. 10k cases.
- S8: `recover_matches_live_state` — replay from `BlockStore` produces the identical state to live execution. 10k cases.

## Plan

S7 + S8 built in one branch:

1. **S7 anchor types** (`suwappudb-bridge::anchor::types`)
2. **S7 AnchorLog** (`anchor::log`)
3. **S7 AnchorDispatcher + parity_check** (`anchor::dispatcher`)
4. **S8 Block + BlockStore** (`recovery::block`, `recovery::store`)
5. **S8 Recovery replay** (`recovery::replay`)
6. **Property tests + IQs + spec docs + scripts/cross-parity.sh + closeout**

## Completed

- [x] S7 + S8 implementation across 7 new modules and 39 new lib unit
  tests.
- [x] Property tests:
  - `crates/suwappudb-bridge/tests/cross_parity.rs` — 4 properties (S7
    exit gate + 3 sub-properties)
  - `crates/suwappudb-bridge/tests/recovery.rs` — 3 properties (S8 exit
    gate + 2 sub-properties)
- [x] **`scripts/cross-parity.sh` finally has a real implementation.**
  Runs the 10k-case property test in dev or release mode.
- [x] IQ-7: anchor log in-memory + MAC now / Solidity + ECDSA at
  launch.
- [x] IQ-8: block store in-memory now / redb at S8.5.
- [x] `docs/spec/anchor-log.md` and `docs/spec/recovery.md` — full
  spec docs for both sprints.

## S7 EXIT GATE: ✅ MET

10,000 cases of `cross_chain_parity_holds` pass (15s in dev). Three
sub-properties also at 10k: `dispatched_anchors_appear_on_all_chains`,
`parity_detects_tampering`, `anchor_chain_is_linked`.

## S8 EXIT GATE: ✅ MET

10,000 cases of `recover_matches_live_state` pass (126s in dev — the
defence-in-depth tree rebuild on every replay block dominates). Two
sub-properties at 10k: `replay_is_deterministic`,
`tampered_state_root_caught`.

## Workspace test count

178 tests pass workspace-wide (was 132):

- suwappudb-bridge lib: 89 (was 50) — +39 (16 anchor + 23 recovery; counts
  include 4 inline call-dispatch tests already in occ)
- suwappudb-bridge tests/block_executor.rs: 4
- suwappudb-bridge tests/cross_parity.rs: 4 (NEW)
- suwappudb-bridge tests/cross_vm_bundles.rs: 5
- suwappudb-bridge tests/cross_vm_parity.rs: 4
- suwappudb-bridge tests/persistent_e2e.rs: 4
- suwappudb-bridge tests/recovery.rs: 3 (NEW)
- suwappudb-lane: 2
- suwappudb-state: 57
- suwappudb-state tests/state_tree.rs: 6

## Phase-1 status

| Sprint | Status |
|---|---|
| S1 | ✅ Lane separation invariant |
| S2 | ✅ Dual-projection @ 3 layers |
| S3 | ✅ Cross-VM parity @ 10k |
| ~S3.5~ | ❎ Dissolved per IQ-3 |
| S4 | ✅ parallel_equals_sequential @ 10k |
| S5 | ✅ bundle_atomicity @ 10k |
| S6 | ✅ cross_tree_root_agreement @ 10k |
| S7 | ✅ cross_chain_parity_holds @ 10k |
| S8 | ✅ recover_matches_live_state @ 10k |

**All 8 phase-1 sprints closed. Every exit gate is a 10k-case property test.**

## Launch-readiness backlog (parallel to phase-1 close)

Recorded in IQs but not yet implemented:

- IQ-3: real Move VM dialect choice + integration
- IQ-6 / IQ-7: real Verkle commitments + IPA witnesses (S6) +
  Solidity `LTPAnchorRegistry` + ECDSA signatures (S7)
- IQ-8: redb-backed `RedbBlockStore` (S8.5)
- IQ-9 (provisional): snapshot checkpoints for replay
- IQ-4: address-shape EVM 20-byte vs Aptos Move 32-byte
- IQ-5: nonce semantics

These are the things that make phase-1 a real chain on real chains. The
property tests already verify the structural invariants survive any
swap — that's the value the trait abstractions bought.

## Blockers

None for phase-1 close.

## Open questions (carried forward)

- IQ-9: snapshot checkpoints
- DAG / multi-parent block representation (deferred to S9 or absorbed
  into launch readiness)
- Persistent intent log across restarts (covered by S8.5 RedbBlockStore)
