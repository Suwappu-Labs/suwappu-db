# State tree (S6)

## Goal

Per-block commitment to the canonical state. Same state ⇒ same root.
Inclusion and non-inclusion proofs verifiable against a known root.

Phase-1 ships hash-based commitments (BLAKE3); real Verkle (IPA over
banderwagon) is a launch-readiness item per **IQ-6**. Tree shape,
depth, and proof format are Verkle-aligned so the swap is mechanical.

## Types and invariants

### Tree

```rust
pub enum Node {
    Empty,
    Leaf(BalanceSlot),
    Internal(BTreeMap<u8, Box<Node>>),
}

pub struct StateTree { root: Node }

pub struct Commitment(pub [u8; 32]);
```

- 256-ary trie keyed by address bytes, depth 20.
- `BTreeMap<u8, Box<Node>>` for sparse representation: only allocated
  children take space, ordered keys give deterministic commitment input.

### Commitment scheme

```
empty_commitment        = BLAKE3("Suwappudb-TREE/EMPTY")
commit(Empty)           = empty_commitment
commit(Leaf(slot))      = BLAKE3("Suwappudb-TREE/LEAF_" | slot.canonical().to_be_bytes())
commit(Internal(kids))  = BLAKE3("Suwappudb-TREE/INT__"
                                 | for (byte, child) in sorted(kids):
                                     byte (1 byte) | commit(child) (32 bytes))
```

Domain-separated tags prevent cross-type collisions.

### Proof shape

```rust
pub struct ProofStep {
    pub byte: u8,
    pub siblings: BTreeMap<u8, Commitment>,
}

pub struct Proof {
    pub path: Vec<ProofStep>,    // root → leaf
    pub slot: Option<BalanceSlot>,
}
```

- **Inclusion proof:** `path.len() == 20`, `slot = Some(_)`. Verifier
  starts from `commit(Leaf(slot))` and walks up.
- **Absence with early termination:** `path.len() < 20`, `slot = None`.
  The deepest step's byte has no child at that depth in the actual
  tree; verifier excludes that byte from the parent's commitment hash.
- **Absence in empty tree:** `path.len() == 0`, `slot = None`. Verifier
  checks `root == empty_commitment`.

### Invariants

For any state S and tree T = `StateTree::from_state(S)`:

1. `T.root()` is deterministic — depends only on the effective
   `Address → BalanceSlot` map, not insert order.
2. Adding, removing, or modifying any entry changes `T.root()` (with
   overwhelming probability — collision-resistant under BLAKE3).
3. For every `(addr, slot)` ∈ S, `T.proof(addr)` verifies as
   inclusion.
4. For every `addr ∉ S`, `T.proof(addr)` verifies as absence.
5. `T.verify` rejects any proof whose claimed slot doesn't match the
   underlying state (tamper resistance).

## Block integration

```rust
pub struct BlockReport {
    pub outcomes: Vec<TxOutcome>,
    pub iterations: usize,
    pub aborts: usize,
    pub state_root: Commitment,   // S6 addition
}
```

`BlockExecutor::execute_with_registry` rebuilds the tree from full
state after consolidation and stashes the root in the report. Phase-1
simplification: full rebuild per block. S6.5 / S8 introduces
incremental updates with no API change.

## Failure model

- **Tampered slot:** `verify` rejects (returns `false`). Property-
  tested at 10k cases.
- **Falsified path:** `verify` rejects (path bytes must match address).
- **Mutated tree concurrent with proof generation:** out of scope.
  Block executor holds `&mut State` exclusively.

## Tests

### Exit gate

```text
PROPTEST_CASES=10000 cargo test --release --test state_tree
```

10,000 cases of all six properties pass (in dev mode in 366s; release
would be ~30s but disk constraints during S6 forced dev). Properties:

- `root_is_deterministic` — order-invariance to insert sequence
- `replay_produces_same_root` — same input ⇒ same output
- `every_inclusion_proof_verifies` — every inserted addr is provable
- `absence_proof_verifies` — addresses outside the seeded space prove
  absent
- `tampered_slot_rejected` — verify rejects bumped-slot claims
- `cross_tree_root_agreement` — sequential vs from-effective-map
  produce the same root and both verify all inclusions

### Inline unit tests

- `tree::types`: 3 tests
- `tree::commit`: 9 tests (determinism, domain separation, ordering
  invariance, distinct paths, leaf-vs-empty)
- `tree::ops`: 12 tests (round-trip, root determinism, root
  sensitivity, inclusion/absence proofs, tamper resistance, bulk-vs-
  sequential equivalence, many-address)

## Witness size disclosure

BLAKE3-based proofs are large compared to real Verkle:

| Level case            | Phase-1 (BLAKE3)               | Real Verkle (IPA)        |
|-----------------------|--------------------------------|--------------------------|
| Inclusion (worst)     | 20 × 255 × 32 = ~163 KB        | ~200 bytes               |
| Absence (early term)  | depth × 255 × 32, depth ≤ 20   | ~200 bytes               |
| Empty tree            | 0 bytes                        | 0 bytes                  |

Stateless light clients are gated on the real-Verkle swap. Per IQ-6,
this is a launch-readiness item.

## Open questions

- **IQ-7 (provisional):** which Rust Verkle implementation
  (`rust-verkle`, `ipa-multipoint`, hand-rolled). Same shape as IQ-3's
  Move VM choice; same launch-readiness gate.
- **Incremental updates.** Phase-1 rebuilds per block. Production
  needs incremental updates touching only changed paths. S6.5 / S8.
- **Persistent tree storage.** Same — tied to S8 recovery; tree could
  live in a redb table alongside the canonical state.
- **Shared subtrees / dedup.** Two trees that share a subtree should
  be able to share its commitment. Verkle handles this naturally; the
  hash-based version recomputes. Performance question, not correctness.
