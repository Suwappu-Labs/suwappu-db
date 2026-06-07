# Suwappu DB — Backend handoff

For an engineer joining cold. Honest, current as of phase-1 close +
S8.5 partial landing (2026-05-08). Read top to bottom; pin the
"Quick reference" section at the end.

---

## 1. What this is

The **storage and execution substrate** for a chain that runs EVM and
Move side by side over a single canonical state.

**It is not yet a chain.** No consensus, no networking, no RPC, no fee
market, no mempool. Phase-1 closed the substrate; everything that
turns it into a chain is open work.

What works: state types, capability-gated mutation, block-level
parallel execution (Aptos Block-STM in shape), cross-VM intent
bundles, state-tree commitments, multi-chain anchor parity, recovery
via deterministic replay (durable via `RedbBlockStore`). **269
property tests pass at 10,000 cases each on the load-bearing claims.**

What's mocked or stubbed: the EVM, the Move VM, the Verkle tree
commitment scheme, the cross-chain anchor authentication. Each has a
documented swap point and an IQ (Important Question) doc explaining
the call.

## 2. Repo

`https://github.com/suwappu/suwappu-db` (private)

## 3. Get it running in 10 minutes

```bash
git clone git@github.com:suwappu/suwappu-db.git
cd suwappu-db

# Need: rustc 1.75+ via rustup, no other deps
rustup toolchain install stable

# Smoke
cargo build --workspace
cargo test --workspace                    # ~30s, 269 tests
cargo clippy --workspace --all-targets -- -D warnings
./scripts/check-lane-separation.sh        # structural invariant
./scripts/cross-parity.sh --quick         # 256-case anchor parity
./scripts/bootstrap.sh smoke              # all-in-one
```

Expected: 269 tests pass, no warnings. If anything fails, that's a
regression — flag it, don't push past it.

## 4. The 8 invariants this substrate guarantees

Each is a property test running 10,000 cases. **Don't break these.**

| # | Invariant | Test |
|---|---|---|
| 1 | Only `suwappudb-bridge` can mutate state (capability gate) | `scripts/check-lane-separation.sh` |
| 2 | EVM `balanceOf` == Move `Coin::value` for any address (structurally) | `redb_preserves_dual_projection` |
| 3 | Same logical op via EVM-shape vs Move-shape ⇒ same canonical state | `interleaved_evm_move_preserves_invariant` |
| 4 | Block execution is deterministic regardless of thread schedule | `parallel_equals_sequential` |
| 5 | Bundle atomicity — failed step ⇒ bundle as if never ran | `bundle_atomicity` |
| 6 | Same state ⇒ same tree root | `cross_tree_root_agreement` |
| 7 | Anchored chains agree on root, or disagreement is detectable | `cross_chain_parity_holds` |
| 8 | Replay from block store == live execution, bit-for-bit | `recover_matches_live_state` |

## 5. File map

| Path | What | Status |
|---|---|---|
| `crates/suwappudb-state/` | Canonical state, BalanceSlot, BalanceStore, StateTree | working |
| `crates/suwappudb-bridge/` | Capability gate, OCC scheduler, bundles, anchors, recovery | working |
| `crates/suwappudb-lane/` | Untrusted ingest layer | placeholder |
| `crates/suwappudb-bridge/src/vm/` | `MockEvm`, `MockMove` | **mocked** — not real VMs |
| `crates/suwappudb-bridge/src/anchor/` | Multi-chain anchor log + parity | **in-memory + HMAC**, not Solidity |
| `crates/suwappudb-bridge/src/recovery/` | Block store + replay | in-memory + redb (S8.5) |
| `crates/suwappudb-state/src/tree/` | 256-ary trie, BLAKE3 commitments | **BLAKE3, not real Verkle** |
| `crates/suwappudb-state/src/redb_store.rs` | Persistent state via redb | working (dev backend per IQ-1) |
| `docs/architecture/` | Diagrams + walkthrough | start here for the visual tour |
| `docs/spec/` | Per-component specs | precise truth, read for the component you're touching |
| `docs/iq/` | Decision docs | read these before changing any swap point |
| `scripts/` | Verify, bootstrap, lane separation, cross-parity | one-line entry points |

## 6. What's mocked, and what it costs

| Thing | Currently | Real version | Cost of mock |
|---|---|---|---|
| EVM | `MockEvm` (no opcodes) | revm | Cannot run real Solidity yet |
| Move VM | `MockMove` (no bytecode) | TBD per IQ-3 | Cannot run real Move yet |
| Tree commitment | BLAKE3 hash | IPA over banderwagon | Witness sizes are 100x too big for stateless clients |
| Anchor auth | BLAKE3 keyed-MAC | ECDSA / EdDSA | Cannot deploy to real chains |
| Anchor storage | in-memory | Solidity `LTPAnchorRegistry` | Anchors die on process exit |
| Block store | `InMemoryBlockStore` or `RedbBlockStore` | same `BlockStore` trait | redb impl available; pick per use case |

These are tracked in IQs (`docs/iq/`). **The trait surfaces are stable**
— when you swap in the real version, the property tests stay green
by construction.

## 7. What's NOT here at all

These didn't fit phase-1 and have no IQ yet:

- **Consensus.** No notion of "who can produce a block."
- **Networking.** No P2P, no gossip, no sync protocol.
- **JSON-RPC / EVM-compatible API.** No external query layer.
- **Mempool.** Phase-1 doesn't model pending transactions.
- **Fee market.** No gas, no priority fees, no MEV.
- **Genesis / chain config.** Starts from `State::default()`.
- **Reorg handling.** Linear chain only.
- **Validator set + slashing.** Detection works; punishment doesn't.
- **Real Solidity `LTPAnchorRegistry` deployment.**

Each will be its own sprint when picked up. **None of this is "almost
done." It's not started.**

## 8. Where to start contributing

Suggested first tasks, ordered by ramp-up speed:

1. **Read `docs/architecture/data-flow.md`** — end-to-end walkthrough
   with sequence diagrams. ~20 minutes.
2. **Read one IQ doc end to end** (e.g.
   `docs/iq/IQ-3-move-vm-choice.md`) — see how decisions are recorded.
3. **Run the 10k exit-gate tests yourself** so you see the property
   tests in action:
   ```bash
   PROPTEST_CASES=10000 cargo test --workspace --release
   ```
4. **Pick a launch-readiness IQ to own.** Suggested order:
   - **IQ-3** (Move VM dialect) — biggest leverage; cascades into IQ-4
     + IQ-5
   - **IQ-1** (RocksDB swap) — parallel, mechanical
   - **IQ-6** (real Verkle) — unblocks stateless clients
   - **IQ-7** (Solidity anchors + ECDSA) — needs validator-set design
     first
5. **Or pick a not-yet-IQ'd missing piece** (consensus, RPC, mempool)
   and write an IQ for it before you start coding. Format is in
   `docs/iq/IQ-3-move-vm-choice.md` — copy and adapt.

## 9. Workflow rules

From `CLAUDE.md`:

- **No `git rebase`.** Always merge or `pull --no-rebase`. Worktrees
  break under rebase.
- **No "Co-Authored-By" lines** in commit messages.
- Use `cargo`, not `cross` or other wrappers.
- Run `bash scripts/verify.sh` before pushing — same gates as CI.
- New work goes on a feature branch; merge to main with `--no-ff` to
  preserve sprint structure.

If you hit a question that affects more than the file you're editing,
**write a new IQ** before answering it in code. Cost of a 1-page IQ
doc is low; cost of an undocumented architectural choice is high.

## 10. Five files to read first (in order)

1. `docs/architecture/README.md` — high-level architecture
2. `docs/architecture/overview.md` — three-crate split + capability gate
3. `docs/architecture/data-flow.md` — end-to-end pipeline with sequence diagrams
4. `crates/suwappudb-state/src/lib.rs` — read top to bottom
5. `crates/suwappudb-bridge/src/lib.rs` — read top to bottom

After those five, you'll have enough context to be useful in review
and to pick something to own.

---

## Quick reference

### Build / test

```bash
cargo build --workspace
cargo test --workspace                                 # ~30s, 269 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all -- --check
```

### Property tests at the exit-gate strength

```bash
PROPTEST_CASES=10000 cargo test --workspace --release  # all invariants
./scripts/cross-parity.sh                              # S7 only, 10k
PROPTEST_CASES=10000 cargo test -p suwappudb-state --test state_tree
PROPTEST_CASES=10000 cargo test -p suwappudb-bridge --test recovery
```

### Verify everything

```bash
bash scripts/verify.sh        # all gates
bash scripts/bootstrap.sh     # build + test + lane-sep + smoke
./scripts/check-lane-separation.sh
```

### Crate boundaries

```
suwappudb-lane → suwappudb-bridge → suwappudb-state
        \________________↗
         (forbidden — lane cannot import state directly)
```

### Sprint exit gates (one test each, 10,000 cases)

| Sprint | Test name | File |
|---|---|---|
| S2 | `redb_preserves_dual_projection` | `crates/suwappudb-state/src/redb_store.rs` |
| S3 | `interleaved_evm_move_preserves_invariant` | `crates/suwappudb-bridge/tests/cross_vm_parity.rs` |
| S4 | `parallel_equals_sequential` | `crates/suwappudb-bridge/tests/block_executor.rs` |
| S5 | `bundle_atomicity` | `crates/suwappudb-bridge/tests/cross_vm_bundles.rs` |
| S6 | `cross_tree_root_agreement` | `crates/suwappudb-state/tests/state_tree.rs` |
| S7 | `cross_chain_parity_holds` | `crates/suwappudb-bridge/tests/cross_parity.rs` |
| S8 | `recover_matches_live_state` | `crates/suwappudb-bridge/tests/recovery.rs` |

### Common types and where they live

| Type | Crate | Module |
|---|---|---|
| `Address`, `Balance`, `BridgeToken`, `State` | suwappudb-state | `lib.rs` |
| `BalanceSlot`, `EvmBalance`, `MoveCoinValue` | suwappudb-state | `balance_slot` |
| `BalanceStore`, `InMemoryBalanceStore`, `RedbBalanceStore` | suwappudb-state | `store`, `redb_store` |
| `Commitment`, `Node`, `Proof`, `StateTree` | suwappudb-state | `tree` |
| `Bridge`, `Intent`, `RejectReason` | suwappudb-bridge | `lib.rs` |
| `EvmTx`, `MoveTx`, `MockEvm`, `MockMove` | suwappudb-bridge | `vm` |
| `MvStore`, `BlockExecutor`, `BlockReport`, `TxOutcome` | suwappudb-bridge | `occ` |
| `Bundle`, `BundleExecutor`, `ContractRegistry`, `BundleGenerator` | suwappudb-bridge | `bundle` |
| `Anchor`, `AnchorLog`, `AnchorDispatcher`, `ParityResult` | suwappudb-bridge | `anchor` |
| `BlockStore`, `InMemoryBlockStore`, `RedbBlockStore`, `replay` | suwappudb-bridge | `recovery` |

### The 8 IQs

| IQ | Topic | Decision |
|---|---|---|
| [IQ-1](iq/IQ-1-redb-vs-rocksdb.md) | State backend | redb in dev, RocksDB at launch |
| [IQ-2](iq/IQ-2-mock-vms-vs-real-vms.md) | EVM/Move integration | Mocks in S3, real VMs fold into S5 |
| [IQ-3](iq/IQ-3-move-vm-choice.md) | Move VM dialect | Deferred to launch readiness |
| IQ-4 (placeholder) | Address shape | TBD with Move VM choice |
| IQ-5 (placeholder) | Nonce semantics | TBD with Move VM choice |
| [IQ-6](iq/IQ-6-verkle-commitment.md) | Tree commitment | BLAKE3 now / IPA at launch |
| [IQ-7](iq/IQ-7-anchor-parity.md) | Anchor auth + storage | In-memory + MAC now / Solidity + ECDSA at launch |
| [IQ-8](iq/IQ-8-recovery-store-inmemory-vs-redb.md) | Block store | In-memory + `RedbBlockStore` (S8.5 partial) |

### Common pitfalls

| Pitfall | What happens | Fix |
|---|---|---|
| Importing `suwappudb-state` from `suwappudb-lane` | `check-lane-separation.sh` fails | Route through `suwappudb-bridge` |
| Calling `BridgeToken::__for_bridge_only` from outside `suwappudb-bridge` | Compile error | Don't — only the bridge mints tokens |
| `git rebase` in a worktree | History corruption | Use `git merge` or `git pull --no-rebase` |
| `git commit` hangs | Hooks fighting | Prefix with `HUSKY=0` |
| Adding "Co-Authored-By" line | Project rule violation | Don't. See `CLAUDE.md` |
| Disk full during `cargo test --release` | ~10GB target/ dir | `cargo clean`; phase-1 dev mode is fine |

### Where to look first

If you're touching... | Read first
---|---
State / balances | `docs/architecture/dual-projection.md` + `crates/suwappudb-state/src/lib.rs`
Block execution | `docs/architecture/data-flow.md` + `crates/suwappudb-bridge/src/occ/block_executor.rs`
Tree | `docs/spec/verkle-state-tree.md` + `crates/suwappudb-state/src/tree/`
Anchors | `docs/spec/anchor-log.md` + `crates/suwappudb-bridge/src/anchor/`
Recovery | `docs/spec/recovery.md` + `crates/suwappudb-bridge/src/recovery/`
Picking a swap point | the relevant IQ in `docs/iq/`
