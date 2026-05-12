# Insertion: DAG L1 paper, new §7.4

**Where:** end of §7 (Execution Layer), after §7.3
(*Checkpoint-synchronized cross-VM writes*).

---

## 7.4 State substrate: GSX-DB

Construction 1 of §7.3 leaves three implementation primitives
unspecified: (i) the polymorphic balance map of §7.2 as a concrete
type with a verifiable dual-projection property; (ii) the intent
queue Q with the property that no producer outside the bridge can
enqueue; and (iii) the joint state commitment `(Σ_EVM, Σ_Move)` over
which the Authority Ring co-signs at each checkpoint. We instantiate
all three in **GSX-DB**, a Rust workspace of four crates that runs
as the state and execution substrate underneath the consensus
layer of §6.

### 7.4.1 Workspace and lane separation

GSX-DB exposes four crates in a strict dependency lattice:

```text
gsxdb-lane  →  gsxdb-bridge  →  gsxdb-state
       \________________↗
        forbidden — no direct lane → state path
```

The forbidden edge is enforced two ways. First, `gsxdb-state`
exposes only one mutation entry point, `State::apply(&BridgeToken,
&StateChange)`, where `BridgeToken` is a zero-sized type whose sole
constructor is `__for_bridge_only()` and lives behind a crate
boundary callable only from `gsxdb-bridge`. Lane code attempting to
synthesize a `BridgeToken` fails to compile. Second, a build-time
script `scripts/check-lane-separation.sh` rejects any source path in
`gsxdb-lane` that resolves to a symbol in `gsxdb-state`.

We refer to this composition as the **lane-separation invariant**:
the type system makes incorrect data-plane mutations
unrepresentable, and the script makes the prohibition observable in
CI.

### 7.4.2 Polymorphic balance map

Per §7.2 the account-field balance map is one polymorphic field
exposed identically to both virtual machines. The implementation is
the `BalanceSlot` type:

```rust
pub struct BalanceSlot { canonical: u128 }

impl BalanceSlot {
    pub fn evm_balance(&self) -> EvmBalance
        { EvmBalance(self.canonical) }
    pub fn move_coin_value(&self) -> MoveCoinValue
        { MoveCoinValue(self.canonical) }
}
```

A single `u128` carries the canonical value; the two projections
expose the EVM and Move view shapes without serializing through a
bridge. There exists no API surface in `gsxdb-state` by which
`evm_balance().to_u128()` and `move_coin_value().to_u128()` can
return distinct values, and the compiler enforces this property
without runtime checks.

**Proposition 4** (Implementation of Proposition 1). *The
polymorphic balance map of §7.2 is implemented by `BalanceSlot` with
projections `evm_balance` and `move_coin_value`. For every
`BalanceSlot` value and every address, the two projections agree on
the canonical balance by construction; this property is preserved
through `InMemoryBalanceStore` and `RedbBalanceStore` round-trips
and through the BLAKE3-commitment state tree of §7.4.5.*

*Proof.* The canonical balance is held in a single private field
exposed only through projections that read it. Storage backends
serialize the canonical field directly; round-tripping the field
through either backend is the identity. The state tree commits to
the canonical map; the tree root is a deterministic function of the
canonical field. We discharge each leg with a 10,000-case property
test (Table 4): `redb_preserves_dual_projection` (S2),
`cross_tree_root_agreement` (S6),
`interleaved_evm_move_preserves_invariant` (S3). $\square$

### 7.4.3 Cross-VM intent queue

The queue $Q$ of Construction 1 is realized as the `Intent::Call`
variant of the `gsxdb-bridge::Intent` enum, dispatched at block
execution time through a `ContractRegistry` keyed by callee
address. Cross-VM writes lift into the registry, which serializes
their effects into a `Bundle` of `BundleStep`s; bundles execute
atomically through `BundleExecutor`, with rollback on any failed
step. We verify two properties of this design:

- **Bundle atomicity.** A bundle in which any step fails leaves
  state indistinguishable from the bundle never having been
  scheduled. Tested at 10,000 cases by
  `crates/gsxdb-bridge/tests/cross_vm_bundles.rs::bundle_atomicity`.

- **Cross-VM canonical equivalence.** A logically identical
  operation issued in EVM-shape or Move-shape produces the same
  canonical state. Tested at 10,000 cases by
  `cross_vm_parity.rs::interleaved_evm_move_preserves_invariant`.

The bundle abstraction is the implementation site of the
"checkpoint-synchronized batching" of §7.3: writes that cross VM
boundaries enqueue as `BundleStep`s and execute under a single
linearized commit observed identically by both VMs.

### 7.4.4 Optimistic concurrency control

Block execution runs an Aptos Block-STM-derived OCC scheduler
[Block-STM, 2022]. Transactions speculatively execute in parallel
via Rayon; a sequential validator pass detects read-set staleness
and retries flagged transactions. Outcomes depend only on the
input transaction-index order, not on the thread schedule.

**Proposition 5** (Schedule determinism). *For any block of intents
and any starting state, two runs of `BlockExecutor::execute` produce
identical post-states.*

The property `parallel_equals_sequential`
(`tests/block_executor.rs`) verifies this at 10,000 cases.
Determinism is the contract on which the recovery argument of
§7.4.6 rests; if Proposition 5 weakens, recovery weakens.

### 7.4.5 State commitment

We commit to the canonical state at every block via a 256-ary
trie keyed by address bytes, depth 20, with domain-separated leaf
and internal commitments. Phase-1 ships hash-based commitments
(BLAKE3 with the `GSXDB-TREE/{EMPTY,LEAF_,INT__}` tag schedule);
the launch-readiness commitment is IPA over the banderwagon curve,
which is a swap of the `commit_node` function with no impact on
tree shape, traversal, or proof structure. The witness-size
difference is documented in §12 (cryptographic posture).

**Proposition 6** (Tree determinism). *For any state $\Sigma$, the
tree root `StateTree::from_state(Σ).root()` is a deterministic
function of $\Sigma$, independent of insertion order.*

Tested at 10,000 cases by
`crates/gsxdb-state/tests/state_tree.rs::cross_tree_root_agreement`.

The state root is the artifact the Authority Ring co-signs at the
checkpoint boundary $t \equiv 0 \pmod C$ of Construction 1. The
joint commitment is precisely the root of the polymorphic balance
map at that height; the dual-projection invariant ensures that
"joint" is well-defined.

### 7.4.6 Persistent block log and recovery

A block records `(height, parent_hash, state_root, intents)` with a
BLAKE3 canonical encoding hash. Block storage exposes the
`BlockStore` trait with two implementations: `InMemoryBlockStore`
(development) and `RedbBlockStore` (durable, redb-backed; switches
to RocksDB for production scale via the same trait).

Recovery is deterministic replay: walk blocks in height order,
re-execute each through the OCC scheduler, verify that the
re-executed `state_root` equals the recorded `state_root` plus a
defense-in-depth re-tree of the post-block state.

**Proposition 7** (Replay equivalence). *For any sequence of blocks
$B_0, \dots, B_n$ executed live against a starting state $\Sigma_0$,
recording $(\text{height}, \text{parent}, \text{state\_root},
\text{intents})$ for each block, replay against a fresh $\Sigma_0$
produces the identical post-state.*

Tested at 10,000 cases by
`crates/gsxdb-bridge/tests/recovery.rs::recover_matches_live_state`.
Tampered `state_root` is detected by the same test
(`tampered_state_root_caught`).

### 7.4.7 Outbound anchor pipeline

The off-chain side of the cross-chain settlement of §10 is realized
as a per-chain append-only `AnchorLog` and a multi-chain
`AnchorDispatcher`. After each block, the dispatcher writes one
anchor per registered chain — each authenticated under that chain's
key, each linked to the previous anchor on the same chain via a
BLAKE3 parent hash. Cross-chain parity is verifiable via
`parity_check(height)`, which returns `Agreed { state_root }` iff
every registered chain has an anchor at that height with matching
roots and valid MACs.

Phase-1 anchor authentication uses a BLAKE3 keyed-MAC; the
launch-readiness path swaps in ECDSA over secp256k1 (and the
companion LTP paper's ML-DSA-65 on the post-quantum surface), with
no change to the `Anchor`, `AnchorLog`, or `parity_check` types.
Solidity parity fixtures for the on-chain `LTPAnchorRegistry`
described in [LTP Academic, 2026, §7] are pre-positioned in
`crates/gsxdb-bridge/tests/solidity_anchor_parity.rs` and cover the
36 entity-state-machine pairs of LTP §7.3.

### 7.4.8 Summary table

| Paper section | GSX-DB construct | Module path |
|---|---|---|
| §7.2 balance map | `BalanceSlot` + projections | `gsxdb-state::balance_slot` |
| §7.3 intent queue $Q$ | `Intent::Call` + `Bundle` | `gsxdb-bridge::bundle` |
| §7.3 checkpoint commit | `StateTree::from_state` | `gsxdb-state::tree` |
| §6 deterministic execution | OCC `BlockExecutor` | `gsxdb-bridge::occ` |
| §10 outbound anchor | `AnchorDispatcher` | `gsxdb-bridge::anchor` |
| §11 crash recovery | `replay`, `BlockStore` | `gsxdb-bridge::recovery` |
| §7 lane separation | `BridgeToken` capability | `gsxdb-state::lib` |

---

### References to add

```bibtex
@misc{BlockSTM2022,
  author = {Rati Gelashvili and Alexander Spiegelman and others},
  title  = {Block-{STM}: Scaling Blockchain Execution by Turning
            Ordering Curse to a Performance Blessing},
  year   = {2022},
  eprint = {arXiv:2203.06871}
}
```
