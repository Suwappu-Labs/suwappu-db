# CE-MVCC OCC (S4)

## Goal

Concurrent execution of a block of intents that produces the same final
state as sequential one-by-one execution, with the dual-projection
invariant intact at every commit point. Aptos Block-STM in shape,
scoped to balance-only intents for phase-1.

## Types and invariants

### Multi-version data store

```rust
pub type TxnIdx = usize;

pub enum ReadSource {
    Snapshot,            // read fell through to the canonical State
    Version(TxnIdx),     // read returned a per-txn write at this idx
}

pub struct MvStore {
    versions: Mutex<HashMap<Address, BTreeMap<TxnIdx, BalanceSlot>>>,
    snapshot: HashMap<Address, BalanceSlot>,
}
```

Invariants:
- A read at `my_idx` returns the highest-versioned entry strictly below
  `my_idx`, or — if none exists — the underlying canonical `State`
  value as `ReadSource::Snapshot`.
- A write at `my_idx` creates or overwrites the version entry for
  `(addr, my_idx)`.
- `clear_writes(idx)` removes all writes belonging to that txn (used
  when re-executing).
- Reads and writes are atomic per call (single critical section).

### Per-txn read/write set

```rust
pub struct ReadEntry  { pub addr: Address, pub source: ReadSource }
pub struct WriteEntry { pub addr: Address, pub value: BalanceSlot  }

pub struct Txn {
    pub idx: TxnIdx,
    pub read_set: Vec<ReadEntry>,
    pub write_set: Vec<WriteEntry>,
    pub rejected: Option<RejectReason>,
}
```

### Validator

```rust
pub trait Validator { fn is_valid(&Txn, &MvStore) -> bool; }
```

Validation rule (pure function of txn + MV state):

| Observed source     | `highest_writer_below(addr, my_idx)` | Verdict |
|---------------------|--------------------------------------|---------|
| `Snapshot`          | `None`                               | valid   |
| `Snapshot`          | `Some(_)`                            | stale   |
| `Version(v)`        | `Some(c)` where `c == v`             | valid   |
| `Version(v)`        | `Some(c)` where `c != v`             | stale   |
| `Version(_)`        | `None` (cleared)                     | stale   |

A txn whose read set is stale must be re-executed.

### Block executor

```rust
pub struct BlockExecutor;
impl BlockExecutor {
    pub fn execute(self, &mut State, &[Intent]) -> BlockReport;
}

pub struct BlockReport {
    pub outcomes: Vec<TxOutcome>,        // per-tx, in input order
    pub iterations: usize,                // 1 = no aborts
    pub aborts: usize,
}
pub enum TxOutcome { Committed, Rejected(RejectReason) }
```

## Algorithm

```
build MvStore
pending = (0..n).collect()
while pending not empty:
    parallel: for each idx in pending, execute_one(idx)
    sequential: for each idx in 0..n, validate
        if !valid: clear_writes(idx); push idx to next_pending
    pending = next_pending

consolidate: walk MvStore at highest version per address,
             apply each (addr, slot) through State::apply
             with a fresh BridgeToken
```

### Determinism

Outcomes depend only on tx-index ordering, not on the rayon thread
schedule. The validator is a pure function of the recorded read/write
sets and the MV store state at validation time. Re-execution is
deterministic per-txn given the same incoming versions.

### Iteration cap

Block-STM proves logarithmic iterations under random workloads. We
nonetheless cap at `2 * block_len + 4` and panic if exceeded — that
signals an algorithmic bug, not pathological input.

## Storage layout

CE-MVCC OCC layers on top of `BalanceStore`. The MV store is
in-memory; only the consolidated post-block state writes to redb (or
RocksDB in S8). The `aggregates`, `evm_storage`, `evm_nonces`,
`move_resources` reserved tables are unused in S4.

## Failure model

- **Insufficient balance / overflow** — recorded as `TxOutcome::Rejected`
  in the report. State unaffected for that tx.
- **Stale read** — caught by the validator, txn re-executes.
- **Iteration cap** — algorithmic bug; panics with a descriptive
  message.
- **Mutex poisoning** — internal invariant violation; panics with
  context.

## Tests

### Exit gate

```text
PROPTEST_CASES=10000 cargo test --release --test block_executor \
    parallel_equals_sequential
```

10,000 cases of random blocks (0–16 intents) over an 8-address seeded
state. Asserts every address has the same balance after parallel
execution and after sequential `Bridge::submit`. Runs in <1s.

### Sub-properties

- `dual_projection_holds_after_block` — EVM/Move agreement after any block
- `total_supply_preserved` — sum of balances is exactly conserved across
  the whole address space (catches double-credit / drop bugs in the
  retry loop)
- `empty_block_is_identity` — empty block leaves state untouched

### Inline unit tests (`occ::*`)

- 7 mv_store unit tests (snapshot fallthrough, version visibility,
  ordering, `clear_writes`, `finalise`, predecessor lookup)
- 6 txn / validator unit tests (validation across all four matrix
  cells, idempotence)
- 8 block_executor unit tests (empty / single / disjoint parallel /
  conflicting / rejected / parallel-vs-sequential / self-transfer /
  noop)

## Pre-existing bug surfaced

The `parallel_equals_sequential` property test on its first 256-case
run minimised to a self-transfer of amount 1, exposing a pre-existing
bug in `Bridge::submit`: with `from == to`, the credit-write of `to`
overwrites the debit-write of `from`, leaving the address at
`balance + amount`. Block executor handled self-transfer as a no-op
correctly, so the two paths disagreed.

Fix: added a `from == to → no-op` guard in `Bridge::submit`, after the
balance check (so the error surface stays consistent for "self-transfer
with insufficient balance"). Plus two regression tests:
`self_transfer_is_a_no_op` and `self_transfer_still_checks_balance`.

This is exactly the kind of latent bug property tests are for. None of
the prior unit tests, integration tests, or S3 cross-VM proptests
caught it because none of them generated self-transfers (S3 used 8
addresses but didn't check post-state sums; only dual-projection,
which holds because both views are equally affected).

## Open questions

- Per-storage-slot read granularity (S5/S6): when contracts arrive,
  the per-address key widens to `(addr, slot)`. The MV store
  generalises naturally; the read-set tuple grows by one field.
- Cross-VM intent invocation (S5): when an EVM call invokes a Move
  resource, both VMs participate in the same tx. Read/write sets must
  span VM boundaries. Surfaces when implemented.
- Persistence of the MV log for recovery (S8): post-mortem replay of
  a block needs the MV history, not just the consolidated post-state.
