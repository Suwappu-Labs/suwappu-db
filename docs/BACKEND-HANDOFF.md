# Backend handoff

For an engineer joining cold. Honest, current as of phase-1 close
+ S8.5 partial landing (2026-05-08). Read this top to bottom before
touching the code.

## TL;DR

This is **the storage and execution substrate** for a chain that runs
EVM and Move side by side over a single canonical state. **It is not
yet a chain.** No consensus, no networking, no RPC, no fee market, no
mempool. Phase-1 closed the substrate; everything that turns it into a
chain is open work.

What works: state types, capability-gated mutation, block-level
parallel execution (Aptos Block-STM in shape), cross-VM intent
bundles, state-tree commitments, multi-chain anchor parity, recovery
via deterministic replay (now also durable via `RedbBlockStore`).
**181 property tests pass at 10,000 cases each on the load-bearing
claims.**

What's mocked or stubbed: the EVM, the Move VM, the Verkle tree
commitment scheme, the cross-chain anchor authentication, the block
storage durability. Each has a documented swap point and an IQ
(Important Question) doc explaining the call.

## Repo

`https://github.com/GlobalSettlementNetwork/gsx-db` (private)

## Get it building in 10 minutes

```bash
git clone git@github.com:GlobalSettlementNetwork/gsx-db.git
cd gsx-db

# Need: rustc 1.75+ via rustup, no other deps
rustup toolchain install stable

# Smoke
cargo build --workspace
cargo test --workspace                    # ~30s, should print 13 "test result: ok" lines
cargo clippy --workspace --all-targets -- -D warnings
./scripts/check-lane-separation.sh        # structural invariant
./scripts/cross-parity.sh --quick         # 256-case anchor parity
./scripts/bootstrap.sh smoke              # all-in-one
```

Expected: 178 tests pass, no warnings. If anything fails, that's a
regression — flag it, don't push past it.

## What's where

| Path | What | Status |
|---|---|---|
| `crates/gsxdb-state/` | Canonical state, BalanceSlot, BalanceStore, StateTree | working |
| `crates/gsxdb-bridge/` | Capability gate, OCC scheduler, bundles, anchors, recovery | working |
| `crates/gsxdb-lane/` | Untrusted ingest layer | placeholder |
| `crates/gsxdb-bridge/src/vm/` | `MockEvm`, `MockMove` | **mocked** — not real VMs |
| `crates/gsxdb-bridge/src/anchor/` | Multi-chain anchor log + parity | **in-memory + HMAC**, not Solidity |
| `crates/gsxdb-bridge/src/recovery/` | Block store + replay | in-memory + redb (S8.5) |
| `crates/gsxdb-state/src/tree/` | 256-ary trie, BLAKE3 commitments | **BLAKE3, not real Verkle** |
| `crates/gsxdb-state/src/redb_store.rs` | Persistent state via redb | working (dev backend per IQ-1) |
| `docs/architecture/` | Diagrams + walkthrough | start here for the visual tour |
| `docs/spec/` | Per-component specs | precise truth, read for the component you're touching |
| `docs/iq/` | "Important Question" decision docs | read these before changing any swap point |
| `scripts/` | Verify, bootstrap, lane separation, cross-parity | one-line entry points |

## The 8 invariants this substrate guarantees

Each is a property test running 10,000 cases. **Don't break these.**

| # | Invariant | Test |
|---|---|---|
| 1 | Only `gsxdb-bridge` can mutate state (capability gate) | `scripts/check-lane-separation.sh` |
| 2 | EVM `balanceOf` == Move `Coin::value` for any address (structurally) | `redb_preserves_dual_projection` |
| 3 | Same logical op via EVM-shape vs Move-shape ⇒ same canonical state | `interleaved_evm_move_preserves_invariant` |
| 4 | Block execution is deterministic regardless of thread schedule | `parallel_equals_sequential` |
| 5 | Bundle atomicity — failed step ⇒ bundle as if never ran | `bundle_atomicity` |
| 6 | Same state ⇒ same tree root | `cross_tree_root_agreement` |
| 7 | Anchored chains agree on root, or disagreement is detectable | `cross_chain_parity_holds` |
| 8 | Replay from block store == live execution, bit-for-bit | `recover_matches_live_state` |

## What's mocked and what it costs you

| Thing | Currently | Real version | Cost of mock |
|---|---|---|---|
| EVM | `MockEvm` (no opcodes) | revm | Cannot run real Solidity yet |
| Move VM | `MockMove` (no bytecode) | TBD per IQ-3 | Cannot run real Move yet |
| Tree commitment | BLAKE3 hash | IPA over banderwagon | Witness sizes are 100x too big for stateless clients |
| Anchor auth | BLAKE3 keyed-MAC | ECDSA / EdDSA | Cannot deploy to real chains |
| Anchor storage | in-memory | Solidity `LTPAnchorRegistry` | Anchors die on process exit |
| Block store | `InMemoryBlockStore` or `RedbBlockStore` | same `BlockStore` trait | redb impl available; pick per use case |

These are tracked in IQs (`docs/iq/`). Each IQ explains the trade-off
and the swap point. **The trait surfaces are stable** — when you swap
in the real version, the property tests stay green by construction.

## What's NOT here at all

These didn't fit phase-1 and have no IQ yet:

- **Consensus.** No notion of "who can produce a block."
- **Networking.** No P2P, no gossip, no sync protocol.
- **JSON-RPC / EVM-compatible API.** No external query layer.
- **Mempool.** Phase-1 doesn't model pending transactions.
- **Fee market.** No gas, no priority fees, no MEV.
- **Genesis / chain config.** Starts from `State::default()`.
- **Reorg handling.** Linear chain only.
- **Validator set + slashing.** Detection of divergent anchors works;
  punishment doesn't exist.

Each will be its own sprint when picked up. **None of this is "almost
done." It's not started.**

## Where to start contributing

Suggested first tasks, ordered by ramp-up speed:

1. **Read `docs/architecture/data-flow.md`.** End-to-end walkthrough
   with diagrams. ~20 minutes.
2. **Read one IQ doc end to end** (e.g. `docs/iq/IQ-3-move-vm-choice.md`).
   You'll see how decisions are recorded.
3. **Run the 10k exit-gate tests yourself** so you see the property
   tests in action:
   ```bash
   PROPTEST_CASES=10000 cargo test --workspace --release
   ```
4. **Pick a launch-readiness IQ** to own. Suggested order:
   - **IQ-3** (Move VM dialect) — biggest leverage; cascades into IQ-4
     + IQ-5
   - **IQ-1, IQ-8** (durable storage swaps) — parallel, mechanical
   - **IQ-6** (real Verkle) — unblocks stateless-client work
   - **IQ-7** (Solidity anchors + ECDSA) — needs validator-set design
     first
5. **Or pick a not-yet-IQ'd missing piece** (consensus, RPC, mempool,
   etc.) and write an IQ for it before you start coding. The format
   is in `docs/iq/IQ-3-move-vm-choice.md` — copy and adapt.

## Workflow rules (please follow)

From `CLAUDE.md`:

- **No `git rebase`.** Always merge or `pull --no-rebase`. Worktrees
  break under rebase.
- **No "Co-Authored-By" lines** in commit messages.
- **`bun` over `npm`/`tsc`** for any TypeScript work (none in phase-1
  yet, but coming).
- Use `cargo`, not `cross` or other wrappers.
- Run `bash scripts/verify.sh` before pushing — same gates as CI.
- New work goes on a feature branch; merge to main with `--no-ff` to
  preserve sprint structure.

## How to ask questions

The IQs (`docs/iq/`) are how decisions get recorded. If you hit a
question that affects more than the file you're editing — write a new
IQ before answering it in code. Pattern:

1. State the question and current context
2. List the options you considered
3. Pick one with reasoning
4. Document consequences and what's left open
5. Add a propagation checklist

If you're not sure whether a question rises to IQ level: it does. Cost
of a 1-page IQ doc is low; cost of an undocumented architectural choice
that two people then disagree about is high.

## Five files to read first (in order)

1. `docs/architecture/README.md` — high-level architecture
2. `docs/architecture/overview.md` — three-crate split + capability gate
3. `docs/architecture/data-flow.md` — end-to-end pipeline with sequence diagrams
4. `crates/gsxdb-state/src/lib.rs` — read top to bottom
5. `crates/gsxdb-bridge/src/lib.rs` — read top to bottom

After those five files you'll have enough context to be useful in
review and to pick something to own.

## Contact / state of the codebase

- Phase-1 closed: 2026-05-08
- 178 tests, 8 invariants, 8 IQs, 7 spec docs
- All sprints merged to `main` with `--no-ff` so sprint history is
  preserved in `git log --oneline --graph`
- Every exit-gate test runnable via `cargo test`. The 10k-case versions
  via `PROPTEST_CASES=10000 cargo test --release`
- No CI yet — see `.github/workflows/` (placeholder); first concrete
  task could be wiring this up

If something here is wrong or stale, fix it in the same PR you're
opening.
