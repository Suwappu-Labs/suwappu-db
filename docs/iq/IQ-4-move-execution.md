# IQ-4: Move VM Execution Strategy

**Status:** Closed (S9)  
**Decision:** Dual-executor architecture with mock (Phase 1) and production (S9+)

---

## Problem Statement

Suwappu-DB must execute Move bytecode to validate that state transitions respect Move invariants. Move is a stack-based VM with its own bytecode format, resource model, and execution semantics. The question: how and when do we introduce real Move execution?

**Launch readiness constraint:** Move execution must not block Phase 1 (S1–S8). Phase 1 uses a mock executor that never fails and always succeeds with input state.

---

## Design: Dual-Executor Architecture

### Phase 1 (S1–S8): MockMoveExecutor

Default executor. Implements the minimal semantics:

```rust
fn execute(&self, addr: &MoveAddress, initial: BalanceSlot) -> ExecutionOutcome {
    ExecutionOutcome::Success {
        coin_value: initial.move_coin_value(),
        sequence: initial.nonce(),
    }
}
```

**Why:** Allows property tests and parity checks to pass without depending on a real Move VM. The mock executor is deterministic and never fails, which simplifies testing and keeps the surface small.

**Feature gate:** None (default behavior).

### Phase 2 (S9+): AptosMoveExecutor

Real Aptos move-vm-runtime executing compiled Move bytecode. Requires:

1. **Module cache:** Loaded from block artifact archive at initialization
2. **Bytecode resolution:** Address → Move module compiled to Aptos canonical format
3. **Interpreter session:** Constructed per-execution with the module cache
4. **Entry point execution:** Call the canonical coin-value extractor function
5. **Error handling:** Map executor errors (abort, OOG) to `ExecutionOutcome::Failure`

**Feature gate:** `production-move-executor` in `Cargo.toml`.

**Integration point:** `suwappudb-bridge/src/vm/executor.rs` (not in state crate).

---

## Trait Design: MoveExecutor

```rust
pub trait MoveExecutor: Debug {
    fn execute(&self, addr: &MoveAddress, initial: BalanceSlot) -> ExecutionOutcome;
}

pub enum ExecutionOutcome {
    Success { coin_value: MoveCoinValue, sequence: AccountNonce },
    Failure,
}
```

**Invariant:** The result (on success) must satisfy:
- `coin_value` is the account's Move `Coin::value` after execution
- `sequence` is the account's sequence number after execution
- Both must agree with the corresponding EVM projections (Proposition 1)

---

## Nonce Semantics in Execution

Move sequence numbers (`account.sequence_number`) are incremented by the Move VM on transaction acceptance. For Suwappu-DB:

1. **Canonical nonce field:** Stored in `BalanceSlot::nonce` (shared between both VMs)
2. **Projection at read time:** 
   - `EvmView::nonce()` → EVM nonce (same numeric value)
   - `MoveView::sequence_number()` → Move sequence (same numeric value)
3. **Projection at write time:** 
   - On transition to `Move::Coin::sequence_number`, must match canonical nonce
   - On transition to `EVM::nonce`, must match canonical nonce

**Dual-projection invariant (Proposition 1, extended S9):**
```
∀ addr:  EVM::nonce(addr) == Move::sequence_number(addr) == canonical_nonce(addr)
```

---

## Execution Flow in Bridge

When `suwappudb-bridge` applies a Move transaction:

1. Resolve the Move address (`MoveAddress`) from the EVM sender
2. Read current state: `slot = state.slot_of(addr)`
3. Invoke executor: `outcome = executor.execute(&move_addr, slot)`
4. On success:
   - Validate `outcome.sequence` matches expected next nonce
   - Apply balance change to canonical slot
   - Update nonce to `outcome.sequence.next()`
5. On failure:
   - Reject transaction with `TransactionError::MoveBytecodeExecution`

---

## Aptos Move VM Integration (S9)

When implementing `AptosMoveExecutor`:

### Dependencies

Add to `suwappudb-bridge/Cargo.toml`:

```toml
aptos-core = { version = "0.1.x", features = ["move-vm-runtime"] }
aptos-types = "0.1.x"
aptos-config = "0.1.x"
move-core-types = "0.1.x"
```

### Implementation Sketch

```rust
pub struct AptosMoveExecutor {
    runtime: Arc<MoveVM>,
    module_cache: Arc<ModuleCache>,
}

impl MoveExecutor for AptosMoveExecutor {
    fn execute(
        &self,
        addr: &MoveAddress,
        initial: BalanceSlot,
    ) -> ExecutionOutcome {
        // 1. Resolve Move module for addr
        let module = match self.module_cache.get(addr) {
            Some(m) => m,
            None => return ExecutionOutcome::Failure,
        };

        // 2. Construct interpreter session
        let session = self.runtime.new_session(module)?;

        // 3. Load initial coin value as argument
        let args = vec![MoveValue::U128(initial.canonical())];

        // 4. Execute canonical coin-value function
        // (e.g., `AptosCoin::coin_value_internal`)
        match session.execute_function(
            &APTOS_COIN_MODULE,
            "coin_value_internal",
            &args,
        ) {
            Ok(return_vals) => {
                let coin_value = extract_u128(&return_vals[0])?;
                // Sequence number is deterministic from Aptos block height
                let sequence = initial.nonce().next();
                ExecutionOutcome::Success {
                    coin_value: MoveCoinValue(coin_value),
                    sequence,
                }
            }
            Err(_) => ExecutionOutcome::Failure,
        }
    }
}
```

### Testing Strategy

1. **Unit tests:** Mock executor tests in `suwappudb-state` (done, S9)
2. **Integration tests:** Load real Aptos bytecode, execute against known coin values
3. **Property tests:** `execute_matches_balance_slot` — all sequences of deposits/withdrawals, real executor output matches canonical slot
4. **Parity tests:** Aptos move-vm-runtime output matches Solidity EVM output for same logical transfer

---

## Feature Gates and Build Configurations

| Config | Executor | Use Case |
|--------|----------|----------|
| Default (no feature) | MockMoveExecutor | Phase 1, dev, test |
| `--features production-move-executor` | AptosMoveExecutor | S9+, mainnet |

CI/CD:
- Fast path: `cargo test` (mock)
- Slow path (main branch only): `cargo test --features production-move-executor` (real VM)

---

## Open Questions for S9

1. **Module cache eviction:** How long do we cache modules? Per-block or session-lifetime?
2. **Out-of-gas semantics:** What's the gas budget per execution? Aptos default?
3. **Custom functions:** Do we define custom Move entry points, or mirror Aptos stdlib?
4. **Cross-VM function calls:** Can an EVM transaction invoke a Move function? How?

These are design decisions for the S9 bridge integration; executor trait is agnostic to them.

---

## Exit Gate for S9

- [x] Mock executor trait + default implementation with property tests
- [x] Nonce semantics wired into projectors (Proposition 1 extended)
- [x] Feature gate in place (production-move-executor)
- [ ] AptosMoveExecutor stubbed with TODO comments
- [ ] Aptos core dependency resolved (blocked on aptos-core versioning)
- [ ] Real executor property tests pass (10k iterations)
- [ ] Parity against Aptos move-vm-runtime verified

Status at S9 close: **executor trait closed; Aptos integration queued for S9.5 hardening phase**.
