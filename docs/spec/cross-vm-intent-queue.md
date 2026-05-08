# Cross-VM intent queue (S5)

## Goal

Atomic cross-VM operations: a single user intent can produce a sequence
of state changes spanning EVM-shape and Move-shape transactions, and
either all commit or none commit. This is the operational form of the
chain's "EVM contract calls a Move resource" claim — without bridges,
without wrapping, just bundles that share the canonical state.

## Types and invariants

### Bundle

```rust
pub enum BundleStep {
    Evm(EvmTx),
    Move(MoveTx),
}

pub struct Bundle {
    pub steps: Vec<BundleStep>,
}
```

Both step flavours reduce to the canonical `Intent::Transfer` via the
existing `to_canonical()` helpers. The bundle layer preserves the VM
tag so reports and tracing can attribute side effects.

### Atomicity contract

For any bundle B applied to state S:

1. If every step of B succeeds, post-state = S' where each step's
   write-set has been merged in order.
2. If any step fails, post-state = S (exactly).
3. Step `n+1` sees step `n`'s writes (intra-bundle reads).
4. Outside the bundle, no intermediate state is observable.

### Bundle result

```rust
pub enum BundleOutcome {
    Committed,
    Reverted { failed_step: usize },
}

pub struct BundleResult {
    pub step_outcomes: Vec<TxOutcome>,
    pub outcome: BundleOutcome,
}
```

### Contract registry

```rust
pub trait BundleGenerator: Send + Sync {
    fn generate(&self, ctx: &CallCtx) -> Bundle;
}

pub struct ContractRegistry {
    by_address: HashMap<Address, Arc<dyn BundleGenerator>>,
}

pub struct CallCtx<'a> {
    pub caller: Address,
    pub target: Address,
    pub value: u128,
    pub calldata: &'a [u8],
    pub state: &'a State,
    pub depth: u8,
}
```

A "contract" is a closure registered by address. Per IQ-3, this is
mock substrate; real revm and real Move drop in via the same trait
when those land.

### Intent extension

```rust
pub enum Intent {
    Transfer { from, to, amount },
    Call {
        caller: Address,
        target: Address,
        value: u128,
        calldata: Vec<u8>,
    },
}
```

Intent is no longer `Copy` (the `Vec<u8>` calldata). `Bridge::submit`
returns `RejectReason::CallRequiresRegistry` for `Call` — only the
block executor dispatches calls, because dispatch needs registry access.

## Storage layout

Bundles only touch the canonical balance map. Reserved tables
(`evm_storage`, `move_resources`) come into play in S6 when contract
storage and Move resource trees land.

## Failure model

- **Step rejection** propagates: any step's `RejectReason` reverts the
  whole bundle. `step_outcomes` retains the reason at the failing
  index; subsequent steps are not attempted.
- **Atomicity at two layers:**
  - `BundleExecutor` (standalone): save-and-restore on the canonical
    `State`. Pre-bundle snapshot of every touched address; on revert,
    re-apply snapshot.
  - `BlockExecutor` with `Intent::Call`: the bundle becomes one OCC
    tx-index; bundle steps' writes accumulate at that idx in the MV
    store. On any step's rejection, `mv.clear_writes(idx)` wipes the
    whole bundle and the OCC machinery sees a rejected txn.
- **Recursion** disallowed in phase-1. `CallCtx::depth` is always 0;
  the block executor refuses to dispatch a call whose generator emits
  another `Intent::Call` step. Real-VM integration revisits when call
  graphs become semantically meaningful.

## Tests

### Exit gate

```text
PROPTEST_CASES=10000 cargo test --release --test cross_vm_bundles \
    bundle_atomicity
```

10,000 cases of random bundles (0–8 mixed EVM and Move steps) over an
8-address seeded state. Asserts atomicity: reverted bundles leave state
exactly as if they never executed; committed bundles preserve total
supply. Runs in <0.2s.

### Sub-properties

- `bundle_equivalence_to_sequential` — committed bundle ≡ stepwise
  sequential application
- `dual_projection_holds_across_bundles` — invariant survives
- `total_supply_preserved_across_bundles` — sum invariant under any
  bundle sequence
- `first_step_failure_is_pure_noop` — boundary: no speculative writes
  to leak

### Inline unit tests

- `bundle::types`: 4 tests
- `bundle::executor`: 8 tests (empty, single, cross-VM, mid-revert,
  first/last fail, step-by-step outcome recording)
- `bundle::registry`: 4 tests (empty, register+lookup, closure
  conformance, re-register)
- `occ::block_executor` Call dispatch: 4 tests (unregistered fallback,
  forwarder bundle, atomic revert on bundle failure, mixed EVM+Move
  steps in a bundle)

## Pre-existing semantics surfaced

The bundle execution path in the OCC framework needed a per-bundle
local accumulator — the MV store deliberately excludes same-idx writes
from `read` (so a txn doesn't see its own writes through the public
API; that's important for OCC validation). Bundle step `n+1` must see
step `n`'s writes, so the executor maintains an in-bundle
`HashMap<Address, BalanceSlot>` consulted before MV. Only MV-resolved
reads enter the OCC read set.

## Open questions

- **Real revm contract calls (deferred to S5.5 / when revm lands per
  IQ-3 retrospective).** Mock generators model the observable behaviour
  but not bytecode-level semantics.
- **Recursive calls.** Phase-1 forbids depth > 0. Real-VM integration
  must support recursion; `CallCtx::depth` is the substrate.
- **Cross-bundle parallelism with shared contracts.** Two Calls in the
  same block to the same contract: each runs the generator
  independently; the two Calls are independent OCC tx-indices.
  Conflicts surface through the read/write set machinery as usual.
- **Persistent intent queues across blocks (S8 recovery).** S5 is
  in-memory only.
