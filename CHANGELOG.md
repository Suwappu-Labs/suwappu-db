# Changelog

All notable changes to suwappu-db are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once the project is tagged. Pre-1.0, minor bumps may include
breaking changes — see `INTEGRATORS.md` "Stability promises".

## [Unreleased]

## [0.1.0-pre] — 2026-05-16

First Phase-1 launch-readiness pre-release. Marks completion of
Pass A (S8.5–S12 sprints), Pass B (security audit + hardening),
and Pass C (external-dev readiness; in-flight at tag time).

### Added — Pass A (Phase-1 launch readiness)

- **S8.5** — Redb-backed `RedbBlockStore` + replay persistence
  hardening (IQ-8).
- **S9** — Real Aptos Move VM via `move-vm-runtime` (IQ-3/4/5);
  `aptos_move_vm_parity` 10k cross-VM proptest under
  `production-move-executor`.
- **S10** — Real Verkle commitments via banderwagon + per-step IPA
  witnesses (IQ-6); `verkle_parity` exit gate under
  `production-verkle`. Compact multipoint IPA witness (~200 B
  target) is an explicit follow-on.
- **S11** — Solidity `LTPAnchorRegistry` + ECDSA parity (IQ-7) —
  `VerifierConfig`, `AnchorLog (anchor, credential)` storage,
  `EcdsaSecp256k1Signer`, `dispatch_with_signer`, Foundry deploy
  script + ABI publication, 16-vector cross-impl differential
  test. Sp1 producer + ML-DSA-65 hybrid follow when zkVM / PQ
  decisions land.
- **S12** — DAG store traversal (children index +
  ancestors/descendants/tips), snapshot capture+restore
  (sorted-encode for byte-idempotent round-trips), Prometheus
  exporter with summary-quantile output, shadow-testnet E2E gated
  on `SUWAPPUDB_SHADOW_RPC`; `dag_snapshot_exit_gate` 10k proptest
  (IQ-9).

### Added — Pass B (security audit + hardening)

- **B2** — Panic sweep: `parse_address_param` in `suwappudb-server`
  RPC handlers; `RedbBlockStore::open` returns typed
  `BlockStoreError` instead of `.expect()`-panicking on corrupt
  redb state.
- **B3** — CI security gates: `clippy --workspace -- -D warnings`,
  `cargo audit --deny warnings`, gitleaks secret-scan (PR + push +
  nightly cron).
- **B5** — Key-custody operational-enforcement clarification.
- **B6** — Opt-in bearer-token middleware (`SUWAPPUDB_BEARER_TOKEN`),
  constant-time compare; deployment-topology doc + nginx /
  Cloudflare Access samples.
- **B7** — Anchor surface deep-review; 12 findings ✅, 3
  documented divergences ⚠ accepted.

### Added — Pass C (external-dev readiness)

- **C1** — `LICENSE` (Apache-2.0) + `NOTICE` at repo root.
- **C2** — `.github/workflows/release.yml` builds Linux + macOS
  binaries + Solidity ABIs on `v*` tags and drafts a GitHub
  Release. Workspace version bumped 0.0.1 → 0.1.0-pre.

Remaining C-items (C3 INTEGRATORS, C4 RPC versioning, C5
suwappudb-types, C6 CONTRIBUTING, C7 ABI cross-ref, C8 distribution)
land on their own branches before the `v0.1.0-pre` tag is cut.

## [Pre-Pass-A entries below]

### Added

- **S7 — Cross-chain anchor log + parity.**
  - `suwappudb-bridge::anchor` module: `Anchor`, `AnchorLog`,
    `AnchorDispatcher`, `parity_check`. In-memory + BLAKE3 keyed-MAC
    per IQ-7.
  - Per-chain append-only logs with parent-hash linkage; tampering is
    detectable.
  - `dispatch(height, state_root)` writes one anchor per registered
    chain.
  - `parity_check(height)` returns `Agreed { state_root }` iff every
    chain's anchor at that height matches; `Disagreed { divergent,
    missing }` otherwise.
  - **Exit-gate test:** `cross_parity::cross_chain_parity_holds` —
    10,000 cases pass (15s dev).
  - **`scripts/cross-parity.sh`** — finally has a real
    implementation (runs the 10k-case property test).
  - **`docs/spec/anchor-log.md`** — full spec doc.
- **IQ-7.** Anchor log is in-memory + MAC in phase-1; Solidity
  `LTPAnchorRegistry` + ECDSA signatures at launch readiness.

- **S8 — Block store + recovery via deterministic replay.**
  - `suwappudb-bridge::recovery` module: `Block`, `BlockHash`,
    `BlockStore` trait + `InMemoryBlockStore`, `replay`,
    `RecoveryError`.
  - Block hash = BLAKE3 of canonical encoding (height, parent, state
    root, intent count, intents).
  - `replay` walks blocks in height order, re-executes via
    `BlockExecutor`, verifies recorded `state_root` matches computed
    + cross-checks via fresh `StateTree::from_state` rebuild.
  - Errors: `StateRootMismatch`, `HeightGap`, `ParentHashMismatch`.
  - **Exit-gate test:** `recovery::recover_matches_live_state` —
    10,000 cases pass (126s dev — defence-in-depth tree rebuild
    dominates).
  - **`docs/spec/recovery.md`** — full spec doc.
- **IQ-8.** Block store is in-memory in phase-1; `RedbBlockStore`
  lands in S8.5 before any deployment.

- **S6 — State-tree commitment.**
  - `suwappudb-state::tree` module: 256-ary trie over
    `Address → BalanceSlot` with BLAKE3-based commitments per IQ-6.
    Verkle-aligned shape (same depth, traversal, proof format) so
    real Verkle (IPA over banderwagon) is a single-function swap.
  - `Node`, `Commitment`, `Proof`, `ProofStep` types.
  - `commit_node` — domain-separated BLAKE3 commitments
    (`SUWAPPUDB-TREE/EMPTY` / `LEAF_` / `INT__`).
  - `StateTree::{new, from_entries, from_state, update, get, root,
    proof, verify}`.
  - Variable-length proofs: full-depth for inclusion, early-termination
    for absence, empty for empty-tree absence.
  - **Exit-gate test:** `cross_tree_root_agreement` — 10,000 cases
    pass (in dev: 366s). Sub-properties: determinism, replay
    equivalence, every inclusion verifies, absence verifies, tamper
    resistance.
  - `BalanceStore::entries()` extension with `InMemory` and `Redb`
    impls.
  - `BlockReport::state_root` — populated by `BlockExecutor` via
    `StateTree::from_state` after consolidation.
  - **`docs/spec/verkle-state-tree.md`** — full spec doc.
- **IQ-6.** State-tree commitment is BLAKE3 in phase-1; real Verkle
  (IPA over banderwagon, ~200-byte witnesses) is a launch-readiness
  item parallel to IQ-3's Move VM choice. Witness-size caveat
  documented; stateless-client work gated on the swap.

- **S5 — Cross-VM intent bundles.**
  - `suwappudb-bridge::bundle` module: `Bundle` (Vec<BundleStep>) with
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
  - `suwappudb-bridge::occ::mv_store`: multi-version balance store with
    per-address `BTreeMap<TxnIdx, BalanceSlot>`. Reads return the
    highest-versioned write strictly below `my_idx`, falling through
    to canonical `State` on cold-cache miss.
  - `suwappudb-bridge::occ::txn`: per-tx read/write sets and a stateless
    OCC `Validator`. Snapshot-vs-Version cell matrix drives
    staleness detection.
  - `suwappudb-bridge::occ::block_executor`: rayon-parallel speculative
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
  - `suwappudb-state::vm` module: `EvmTx` / `MoveTx` typed transaction
    shapes both reducing to `CanonicalTransfer` via `to_canonical()`.
  - `EvmProjector` / `MoveProjector` traits + `EvmView` / `MoveView`
    default impls that read via `State::slot_of` and project the
    canonical `BalanceSlot`.
  - `suwappudb-bridge::vm::executor`: `MockEvm` / `MockMove` faithful Rust
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
  - Three core crates: `suwappudb-state`, `suwappudb-bridge`, `suwappudb-lane`.
  - Capability-token gate: `State::apply` requires a `BridgeToken` that
    only `suwappudb-bridge` can mint.
  - `scripts/check-lane-separation.sh` blocks `suwappudb-lane` from importing
    or depending on `suwappudb-state`. Verified to catch both `Cargo.toml`
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

[Unreleased]: https://github.com/suwappu/suwappu-db/commits/main
