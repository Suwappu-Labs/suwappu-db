# Changelog

All notable changes to gsx-db are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once the project is tagged. Pre-tag, every change lands under `[Unreleased]`.

## [Unreleased]

### Added

- **S5 — Cross-VM intent bundles.**
  - `gsxdb-bridge::bundle` module: `Bundle` (Vec<BundleStep>) with
    `BundleStep::{Evm, Move}` and `BundleResult` /
    `BundleOutcome::{Committed, Reverted{failed_step}}`.
  - `BundleExecutor::execute(&mut State, &Bundle)` — standalone
    save-and-restore atomicity. Snapshot every touched address; on
    revert, restore.
  - `ContractRegistry` + `BundleGenerator` trait + `CallCtx`:
    address-keyed mock-contract substrate. Closures with the right
    shape are generators automatically. Per IQ-3, real revm and real
    Move drop into the same trait when those land.
  - `Intent::Call { caller, target, value, calldata }` variant.
    `Intent` is no longer `Copy`. `Bridge::submit` returns
    `RejectReason::CallRequiresRegistry` for `Call`.
  - `BlockExecutor::execute_with_registry` — dispatches `Intent::Call`
    within OCC: registry lookup, generator runs, bundle steps execute
    atomically at one OCC tx-index. Per-bundle local accumulator
    lets step `n+1` see step `n`'s writes.
  - **Exit-gate test:** `cross_vm_bundles::bundle_atomicity` —
    10,000 cases in release pass in 0.14s. Sub-properties
    (`bundle_equivalence_to_sequential`,
    `dual_projection_holds_across_bundles`,
    `total_supply_preserved_across_bundles`,
    `first_step_failure_is_pure_noop`) also pass at 10k.
  - **`docs/spec/cross-vm-intent-queue.md`** — full spec doc.

- **S4 — CE-MVCC OCC (Aptos Block-STM style).**
  - `gsxdb-bridge::occ::mv_store`: multi-version balance store with
    per-address `BTreeMap<TxnIdx, BalanceSlot>`. Reads return the
    highest-versioned write strictly below `my_idx`, falling through
    to canonical `State` on cold-cache miss.
  - `gsxdb-bridge::occ::txn`: per-tx read/write sets and a stateless
    OCC `Validator`. Snapshot-vs-Version cell matrix drives
    staleness detection.
  - `gsxdb-bridge::occ::block_executor`: rayon-parallel speculative
    execution + sequential validation + clear-and-retry loop. Cap of
    `2*n+4` iterations. Consolidation via fresh `BridgeToken`.
  - **Exit-gate test:** `block_executor::parallel_equals_sequential`
    — 10,000 cases in release pass in 0.54s. Three sub-properties
    (`dual_projection_holds_after_block`, `total_supply_preserved`,
    `empty_block_is_identity`) also pass at 10k.
  - **`docs/spec/ce-mvcc-occ.md`** — full spec doc.
  - New dep: rayon.

### Fixed

- **`Bridge::submit` self-transfer bug.** With `from == to`, the
  credit-write of `to` overwrote the debit-write of `from`, leaving
  the address at `balance + amount`. Surfaced by S4's
  `parallel_equals_sequential` proptest, which minimised to a
  self-transfer of amount 1. Fix: explicit `from == to → no-op` guard
  in `Bridge::submit`, after the balance check. Two regression tests
  added (`self_transfer_is_a_no_op`, `self_transfer_still_checks_balance`).

- **IQ-3 — Move VM choice deferred.** The Move dialect (Aptos / Sui /
  upstream / hand-rolled) is reframed as a launch-readiness decision,
  not a phase-1 sprint deliverable. Phase-1 ships with `MockMove`
  through S8; real Move VM integration lands when the chain is
  preparing for testnet/mainnet. Hand-rolled minimal interpreter is
  the contingency if a sprint forces the question first.
- **S3.5 dissolved.** Real-revm integration folds into S5 (cross-VM
  intent queue) where contract calls give it real bug-finding value.

- **S3 — EVM + Move projector wiring.**
  - `gsxdb-state::vm` module: `EvmTx` / `MoveTx` typed transaction
    shapes both reducing to `CanonicalTransfer` via `to_canonical()`.
  - `EvmProjector` / `MoveProjector` traits + `EvmView` / `MoveView`
    default impls that read via `State::slot_of` and project the
    canonical `BalanceSlot`.
  - `gsxdb-bridge::vm::executor`: `MockEvm` / `MockMove` faithful Rust
    mock executors, both routing through `Bridge::submit`. EVM revert
    / Move abort error semantics modelled.
  - **Exit-gate test:** `cross_vm_parity::interleaved_evm_move_preserves_invariant`
    — 10,000 cases in release pass in 0.17s. The dual-projection
    invariant holds under arbitrary mixed-VM transaction sequences.
  - Three sub-properties: EVM-only, Move-only, and encoding-symmetry on
    independent states.
- **IQ-2.** S3 ships with mock executors; real revm + Move VM
  integration deferred to S3.5 because no clean standalone Move VM
  crate exists today (Aptos's pulls the framework, Sui's is forked
  into Sui).
- **`docs/spec/dual-vm-projectors.md`** — first real spec doc, lifts
  S3's types, executor wiring, invariant, and exit-gate test into
  prose.

- **S2 — Persistent dual-projection invariant.**
  - `BalanceSlot` type with `EvmBalance` / `MoveCoinValue` projections.
    Proposition 1 enforced structurally: one canonical field, two views.
  - `BalanceStore` trait + `InMemoryBalanceStore` impl.
  - `RedbBalanceStore` impl with five tables (`state`, `aggregates`,
    `evm_storage`, `evm_nonces`, `move_resources`) materialised at open.
  - Property tests at three layers — type, in-memory, persistent — all
    passing the dual-projection invariant under arbitrary mutations.
  - `State` refactored to delegate to a pluggable `BalanceStore` backend.
  - End-to-end integration: `Lane → Bridge → State → redb`.
- **IQ-1.** Phase-1 storage backend split: redb in dev/CI, RocksDB in
  production (S8). Decision driven by local build-disk constraints.
- **S1 — Workspace + lane separation.**
  - Three core crates: `gsxdb-state`, `gsxdb-bridge`, `gsxdb-lane`.
  - Capability-token gate: `State::apply` requires a `BridgeToken` that
    only `gsxdb-bridge` can mint.
  - `scripts/check-lane-separation.sh` blocks `gsxdb-lane` from importing
    or depending on `gsxdb-state`. Verified to catch both `Cargo.toml`
    and source-level violations.
- **Workspace lints.** `forbid(unsafe_code)`, `deny(clippy::all)`,
  `warn(clippy::pedantic)` across all crates.
- **Project scaffolding.** `CLAUDE.md`, slash commands (`/sprint`,
  `/check`, `/release`, `/aws-status`, `/audit-bridge`, `/cross-parity`,
  `/iq-decision`), subagents (`lane-auditor`, `crypto-reviewer`,
  `parity-checker`, `sprint-runner`), `claude-code/install.sh`.

### Changed

- _nothing yet_

### Fixed

- _nothing yet_

[Unreleased]: https://github.com/GlobalSettlementNetwork/gsx-db/commits/main
