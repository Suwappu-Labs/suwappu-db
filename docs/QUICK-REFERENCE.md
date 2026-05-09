# Quick reference

One-page cheat sheet. Pin this.

## Build / test

```bash
cargo build --workspace
cargo test --workspace                                 # ~30s, 181 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

## Property tests at the exit-gate strength

```bash
PROPTEST_CASES=10000 cargo test --workspace --release  # all invariants
./scripts/cross-parity.sh                              # S7 only, 10k
PROPTEST_CASES=10000 cargo test -p gsxdb-state --test state_tree
PROPTEST_CASES=10000 cargo test -p gsxdb-bridge --test recovery
```

## Verify everything

```bash
bash scripts/verify.sh        # all gates
bash scripts/bootstrap.sh     # build + test + lane-sep + smoke
./scripts/check-lane-separation.sh
```

## Crate boundaries

```
gsxdb-lane → gsxdb-bridge → gsxdb-state
        \________________↗
         (forbidden — lane cannot import state directly)
```

## Sprint exit gates (one test each, 10,000 cases)

| Sprint | Test name | File |
|---|---|---|
| S2 | `redb_preserves_dual_projection` | `crates/gsxdb-state/src/redb_store.rs` |
| S3 | `interleaved_evm_move_preserves_invariant` | `crates/gsxdb-bridge/tests/cross_vm_parity.rs` |
| S4 | `parallel_equals_sequential` | `crates/gsxdb-bridge/tests/block_executor.rs` |
| S5 | `bundle_atomicity` | `crates/gsxdb-bridge/tests/cross_vm_bundles.rs` |
| S6 | `cross_tree_root_agreement` | `crates/gsxdb-state/tests/state_tree.rs` |
| S7 | `cross_chain_parity_holds` | `crates/gsxdb-bridge/tests/cross_parity.rs` |
| S8 | `recover_matches_live_state` | `crates/gsxdb-bridge/tests/recovery.rs` |

## Common types and where they live

| Type | Crate | Module |
|---|---|---|
| `Address`, `Balance`, `BridgeToken`, `State` | gsxdb-state | `lib.rs` |
| `BalanceSlot`, `EvmBalance`, `MoveCoinValue` | gsxdb-state | `balance_slot` |
| `BalanceStore`, `InMemoryBalanceStore`, `RedbBalanceStore` | gsxdb-state | `store`, `redb_store` |
| `BlockStore`, `InMemoryBlockStore`, `RedbBlockStore`, `replay` | gsxdb-bridge | `recovery` |
| `Commitment`, `Node`, `Proof`, `StateTree` | gsxdb-state | `tree` |
| `Bridge`, `Intent`, `RejectReason` | gsxdb-bridge | `lib.rs` |
| `EvmTx`, `MoveTx`, `MockEvm`, `MockMove` | gsxdb-bridge | `vm` |
| `MvStore`, `BlockExecutor`, `BlockReport`, `TxOutcome` | gsxdb-bridge | `occ` |
| `Bundle`, `BundleExecutor`, `ContractRegistry`, `BundleGenerator` | gsxdb-bridge | `bundle` |
| `Anchor`, `AnchorLog`, `AnchorDispatcher`, `ParityResult` | gsxdb-bridge | `anchor` |
| `Block`, `BlockStore`, `InMemoryBlockStore`, `replay`, `RecoveryError` | gsxdb-bridge | `recovery` |

## The 8 IQs

| IQ | Topic | Decision |
|---|---|---|
| [IQ-1](iq/IQ-1-redb-vs-rocksdb.md) | State backend | redb in dev, RocksDB at launch |
| [IQ-2](iq/IQ-2-mock-vms-vs-real.md) | EVM/Move integration | Mocks in S3, real VMs fold into S5 |
| [IQ-3](iq/IQ-3-move-vm-choice.md) | Move VM dialect | Deferred to launch readiness |
| IQ-4 (placeholder) | Address shape | TBD with Move VM choice |
| IQ-5 (placeholder) | Nonce semantics | TBD with Move VM choice |
| [IQ-6](iq/IQ-6-verkle-vs-hash-commitment.md) | Tree commitment | BLAKE3 now / IPA at launch |
| [IQ-7](iq/IQ-7-anchor-log-onchain-vs-inmemory.md) | Anchor auth + storage | In-memory + MAC now / Solidity + ECDSA at launch |
| [IQ-8](iq/IQ-8-recovery-store-inmemory-vs-redb.md) | Block store | In-memory + `RedbBlockStore` (S8.5 partial) |

## What does NOT exist

- Consensus
- Networking / P2P
- JSON-RPC / EVM-compatible API
- Mempool
- Fee market / gas
- Validator set / slashing
- Genesis configuration
- Reorg handling
- Real EVM (revm not integrated)
- Real Move VM
- Real Verkle commitments
- Real Solidity anchor contract
- Real Solidity `LTPAnchorRegistry` deployment

Each is its own sprint.

## Common pitfalls

| Pitfall | What happens | Fix |
|---|---|---|
| Importing `gsxdb-state` from `gsxdb-lane` | `check-lane-separation.sh` fails | Route through `gsxdb-bridge` |
| Calling `BridgeToken::__for_bridge_only` from outside `gsxdb-bridge` | Compile error | Don't — only the bridge mints tokens |
| `git rebase` in a worktree | History corruption | Use `git merge` or `git pull --no-rebase` |
| `git commit` hangs | Husky hooks fighting | Prefix with `HUSKY=0` |
| Adding "Co-Authored-By" line | Project rule violation | Don't. See `CLAUDE.md` |
| Disk full during `cargo test --release` | ~10GB target/ dir | `cargo clean`; phase-1 dev mode is fine |

## Where to look first

For X, read Y:

| If you're touching... | Read first |
|---|---|
| State / balances | `docs/architecture/dual-projection.md` + `crates/gsxdb-state/src/lib.rs` |
| Block execution | `docs/architecture/data-flow.md` + `crates/gsxdb-bridge/src/occ/block_executor.rs` |
| Tree | `docs/spec/verkle-state-tree.md` + `crates/gsxdb-state/src/tree/` |
| Anchors | `docs/spec/anchor-log.md` + `crates/gsxdb-bridge/src/anchor/` |
| Recovery | `docs/spec/recovery.md` + `crates/gsxdb-bridge/src/recovery/` |
| Picking a swap point | the relevant IQ in `docs/iq/` |
