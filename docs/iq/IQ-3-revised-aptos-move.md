## IQ-3 Revised: Move VM Dialect Decision — Aptos for mainnet

**Status:** Accepted
**Date:** 2026-05-09
**Sprint context:** S9 (launch-readiness), follow-up to IQ-3 (deferred decision)

### Question

Phase-1 deferred the Move VM dialect choice (IQ-3). Now that mainnet launch is a concrete milestone (S9+), which Move VM should GSX-DB integrate?

### Context

IQ-3 identified 5 options:
1. Aptos `move-vm-runtime`
2. Sui's fork (vendored)
3. Upstream `move-language/move`
4. Hand-rolled minimal interpreter
5. Defer (what we chose for Phase-1)

Phase-1 shipped the structural and operational invariants with `MockMove`. Real Move integration is now a launch-readiness blocker for testnet/mainnet.

### Decision

**Aptos `move-vm-runtime` as the primary integration.**

Rationale:
1. **Production-tested scale** — Aptos mainnet runs billions of transactions with this VM.
2. **Ecosystem strength** — Aptos is the largest Move network by TVL and developer adoption.
3. **Build stability** — Published to crates.io; stable API contract; backed by Aptos Foundation.
4. **Maintenance model** — We track Aptos releases; they own the VM updates and security patches.
5. **Framework alignment** — `aptos_framework::coin::Coin<T>` is the de facto standard for Move tokens.
6. **Risk containment** — If this choice becomes untenable later, the hand-rolled fallback (Option 4) is a known retreat.

### Trade-offs

- **Framework lock-in:** GSX-DB's Move code will be Aptos-flavoured (ability set, gas model, native functions). This is acceptable because Aptos is the ecosystem we're launching into.
- **Build size:** ~500MB target directory. Acceptable for production; mitigated in CI via Docker layer caching.
- **Upgrade cadence:** We track Aptos releases. We accept this maintenance responsibility.

### Implementation

- Import `aptos-core` (mainnet-compatible version, currently ~0.7.x stable)
- Replace `MockMove` with `aptos_vm::AptosVM`
- Adapt the `MoveProjector` to use Aptos VM syscalls
- Ensure dual-projection invariant holds under real Move bytecode (property tests)

### Consequences

- `gsxdb-bridge` gains a new `move-vm` feature gate (optional, off by default in tests)
- `Cargo.toml` adds `aptos-core` with locked version
- `MoveProjector::execute` now calls real bytecode, not a mock
- Phase-1 property tests must re-run with real Move to confirm invariants hold
- Launch-readiness checklist: "real Move bytecode validates Proposition 1" becomes exit gate

### What this leaves open

- **IQ-4 (address-shape):** EVM 20-byte vs Aptos 32-byte. Addressed in S9 via `Address` enum.
- **IQ-5 (nonce semantics):** EVM nonce vs Aptos sequence number. Addressed in S9 via projection layer.
- **IQ-9 (snapshot checkpoints):** Deferred to S12.

### Propagation

- [ ] Create `gsxdb-bridge/src/vm/aptos_vm.rs` — AptosVM wrapper
- [ ] Update `MoveProjector` to dispatch to real or mock (feature gate)
- [ ] Add `aptos-core` to `Cargo.toml` with version lock
- [ ] Re-run Phase-1 property tests with real Move (`dual_vm_parity.rs`)
- [ ] Document Move bytecode serialization in `docs/spec/move-execution.md`
