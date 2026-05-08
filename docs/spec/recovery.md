# Block store + recovery (S8)

## Goal

Recover a fresh node's canonical state from a durable block log via
deterministic replay. The post-replay state must be bit-for-bit
identical to the live-execution state at the same height.

Phase-1 ships in-memory block storage per IQ-8. Persistent redb-backed
storage is S8.5; the trait surface is unchanged across the swap.

## Types and invariants

### Block

```rust
pub struct Block {
    pub height: u64,
    pub parent: BlockHash,
    pub state_root: Commitment,
    pub intents: Vec<Intent>,
}
```

Records the inputs to one `BlockExecutor::execute_with_registry`
invocation plus the resulting state-tree root from S6.

`hash()` returns the BLAKE3 of the canonical encoding (height, parent,
state_root, intent count, then each intent in order with type tags).

### BlockStore

```rust
pub trait BlockStore {
    fn put(&mut self, block: Block);
    fn get_by_hash(&self, hash: &BlockHash) -> Option<Block>;
    fn get_by_height(&self, height: u64) -> Option<Block>;
    fn latest(&self) -> Option<Block>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { ... }
    fn iter_from(&self, from: u64) -> Vec<Block>;
}
```

Append-only. Phase-1 ships `InMemoryBlockStore`; S8.5 adds
`RedbBlockStore` (per IQ-8).

### Replay

```rust
pub fn replay(
    store: &dyn BlockStore,
    state: &mut State,
    registry: &ContractRegistry,
    from: u64,
) -> Result<(), RecoveryError>;
```

Walks blocks from `from` (inclusive), re-executes each through
`BlockExecutor`, and verifies the produced `state_root` matches the
recorded one.

### Recovery invariant

For any sequence of blocks executed live, with each `(height, parent,
state_root, intents)` recorded in a `BlockStore`:

```
replay(store, fresh_state, registry, 0) → live_state
```

at every address. The post-replay state equals the live state. Any
tampering with `Block::state_root` in the store causes
`RecoveryError::StateRootMismatch`. Any height gap causes
`RecoveryError::HeightGap`. Any broken parent chain causes
`RecoveryError::ParentHashMismatch`.

## Determinism dependency

Replay correctness depends on `BlockExecutor` being deterministic
given the same input intents and starting state. This is the S4
`parallel_equals_sequential` invariant — outcomes depend only on
tx-index ordering, not on the rayon thread schedule. If S4's
determinism breaks, S8's recovery breaks too. The `recover_matches_live_state`
property test exercises both invariants together at 10k cases.

## Storage layout

Phase-1 in-memory:
- `HashMap<BlockHash, Block>` keyed lookup
- `BTreeMap<u64, BlockHash>` height index

S8.5 redb (per IQ-8):
- Table `blocks_by_hash`: `&[u8] → &[u8]` (32-byte hash → encoded block)
- Table `height_to_hash`: `u64 → &[u8]` (height → 32-byte hash)
- One write txn per `put` covering both tables.

## Failure model

- **State root mismatch**: replay aborts with the divergent height.
  Indicates either a tampered store, a non-deterministic executor
  (S4 regression), or a real-revm/Move VM swap that changed
  semantics.
- **Height gap**: replay aborts. Indicates a missing block in the
  store.
- **Parent hash mismatch**: replay aborts. Indicates a fork or
  tampering.

## Tests

### Exit gate

```text
PROPTEST_CASES=10000 cargo test --test recovery recover_matches_live_state
```

10,000 cases of random block sequences (0–6 blocks, each with 0–6
intents over 8 addresses). Live execution captures `(height, parent,
state_root, intents)`; replay rebuilds the same state from a fresh
seeded store. Every address must match.

### Sub-properties

- `replay_is_deterministic` — same store + same start state ⇒ same
  end state, every time.
- `tampered_state_root_caught` — flipping any block's recorded state
  root causes replay to fail.

### Inline unit tests

- 6 block tests (hash determinism, sensitivity to every field)
- 5 store tests (round-trip, latest, iter_from)
- 5 replay tests (single/multi block reproduction, tamper detection,
  height gap, parent chain)

## Pre-existing semantics surfaced

Replay does a defence-in-depth check: after re-executing each block,
it both compares the executor's reported `state_root` AND recomputes
the tree root via `StateTree::from_state`. The two should always
agree (S6 invariant); if they ever drift, the second check catches
it before the wrong root propagates.

## Open questions

- **DAG variant.** Phase-1 is linear (single parent). The original
  sprint name "DAG store + recovery" leaves room for `parents:
  Vec<BlockHash>` (Narwhal-style). Adding it is a single field
  change with no commitment-scheme impact.
- **Snapshot checkpoints (IQ-9 candidate).** Replay-from-genesis
  scales O(chain length). Production wants periodic state snapshots.
- **Persistent block storage (S8.5).** Per IQ-8, redb backend lands
  before any deployment.
- **Multi-process recovery.** Phase-1 single-process; production may
  recover via parallel block execution across CPU cores.
