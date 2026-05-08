# Dual-VM projectors (S3)

## Goal

Operationalise Proposition 1 — `EVM balanceOf == Move Coin.value` — by
introducing a typed transaction-and-projection layer over the canonical
`BalanceSlot`. The same logical operation, expressed as either an
EVM-shaped or a Move-shaped transaction, must produce identical
canonical state, and reads through the EVM and Move projectors must
agree at every point.

This is the load-bearing test of the dual-VM design.

## Types and invariants

### Transaction shape

```rust
pub struct EvmTx   { pub from: Address,   pub to: Address,        pub value: u128  }
pub struct MoveTx  { pub signer: Address, pub recipient: Address, pub amount: u128 }
```

Both reduce to a canonical `CanonicalTransfer { from, to, amount }` via
`to_canonical()`. The reduction discards VM-specific naming but
preserves the semantic content. Phase-1 only models the transfer
primitive; S5 (cross-VM intent queue) extends to contract calls.

### Read projectors

```rust
trait EvmProjector  { fn balance_of(&State, &Address) -> EvmBalance; }
trait MoveProjector { fn coin_value(&State, &Address) -> MoveCoinValue; }
```

`EvmView` and `MoveView` are the default implementations. Both delegate
to `State::slot_of(addr)` and project the resulting `BalanceSlot`. The
dual-projection invariant is structurally true at this layer because
both views read from the same canonical `u128` field.

### Executors

```rust
pub struct MockEvm;  impl MockEvm  { fn execute(&mut State, EvmTx)  -> Result<(), EvmError>;  }
pub struct MockMove; impl MockMove { fn execute(&mut State, MoveTx) -> Result<(), MoveError>; }
```

Both route through `Bridge::submit(Intent::Transfer)`. They do not
mutate `State` directly. Error types are VM-flavoured (`EvmError::Revert`,
`MoveError::Abort`) but both collapse to "no state change on error"
through the bridge.

### Invariant

For any sequence of mixed EVM and Move transactions applied to a single
canonical state, and for any address `a`:

```
EvmView::balance_of(state, a).to_u128()
  ==
MoveView::coin_value(state, a).to_u128()
  ==
state.slot_of(a).canonical()
```

## Storage layout

S3 only writes to the `state` table (the canonical balance map). The
reserved tables (`evm_storage`, `evm_nonces`, `move_resources`,
`aggregates`) come into play in S5/S6 when contract storage and Move
resource trees land.

## Failure model

**S3 mock executor caveat.** The executors are faithful Rust models of
EVM transfer and Move transfer semantics, not real revm or Move VM
runtimes. This is recorded as **IQ-2**.

What the property test verifies:
- The encoding path (`EvmTx::to_canonical`, `MoveTx::to_canonical`) is
  symmetric and lossless.
- The execution path (`Bridge::submit`) is the only mutation route, and
  routes through the `BridgeToken` capability gate.
- The projection path (`EvmView`, `MoveView`) returns the canonical
  field.

What it does *not* verify:
- Bytecode-level VM divergence (e.g., an EVM opcode disagreeing with
  another EVM opcode for the same account). Real revm catches this; the
  mock does not.
- Cross-VM contract calls (S5).

Real-VM integration is provisionally **S3.5**, scheduled before S5.

## Tests

### Exit gate

```
PROPTEST_CASES=10000 cargo test --release --test cross_vm_parity \
    interleaved_evm_move_preserves_invariant
```

10,000 cases of interleaved EVM+Move transactions over a seeded 8-address
state. After every transaction, the dual-projection invariant is
asserted on every address. Runs in <250ms.

### Sub-properties

- `evm_only_preserves_invariant` — pure EVM workloads
- `move_only_preserves_invariant` — pure Move workloads
- `evm_and_move_canonical_equivalents_match` — encoding-path symmetry
  on independent states

## Open questions

- **IQ-3 (open):** Which Move VM crate (or hand-rolled interpreter)
  for S3.5. Aptos vs Sui vs custom; each has tradeoffs for binary size,
  framework lock-in, and on-chain compatibility.
- **IQ-4 (open):** Address-shape mismatch. EVM is 20-byte; Aptos Move
  is 32-byte. Pad globally or map at the projector layer?
- **IQ-5 (open):** Nonces. EVM uses them for replay protection; Move's
  signing model doesn't use nonces in the same way. Phase-1 mocks
  ignore the question; real revm forces it.
