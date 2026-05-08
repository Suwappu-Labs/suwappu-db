## IQ-2: S3 dual-VM consistency — mock executors in phase-1, real VMs deferred

**Status:** Accepted
**Date:** 2026-05-08
**Sprint context:** S3 (EVM + Move projector wiring)

### Question

S3's exit gate is the dual-projection invariant under load:
`EVM balanceOf == Move Coin.value` after arbitrary mixed-VM transaction
sequences. The strict reading of the spec is that this invariant is
verified against *real* EVM execution (revm) and *real* Move execution
(some Move VM crate).

We need to decide whether S3 ships with real VM execution or with
faithful mocks, and what that means for the chain in production.

### Context

The dual-VM design has a single canonical state (`BalanceSlot`) with two
projections (`EvmBalance`, `MoveCoinValue`). Both projections read from
the same canonical field, so the invariant is structurally true at the
projection layer regardless of which VM produced the mutation.

What S3 actually proves is that the *encoding paths* — `EvmTx → Intent`
and `MoveTx → Intent` — and the *execution paths* through
`Bridge::submit` preserve the invariant. That is, no executor can write
state that one projection sees and the other doesn't.

The question is whether the executors run real VM bytecode or model the
VM semantics in Rust.

### Options considered

1. **Real revm + real Move VM.**
   - Pros: maximal spec fidelity. Bytecode-level semantics exercised.
   - Cons:
     - revm is fine: pure Rust, ~30 deps, builds in seconds.
     - Move VM is the problem. Aptos's `move-vm-runtime` crate pulls the
       entire Aptos framework (~hundreds of crates, large compile). Sui's
       Move VM is forked into the Sui repo and not packaged for external
       use. There is no clean standalone Move VM crate today.
     - Going asymmetric (real revm + mock Move) is worse than symmetric
       — the test would prove "EVM bytecode and a Rust function agree,"
       which is not the property we care about.

2. **Mock executors in pure Rust, modelled on EVM and Move semantics.**
   - Pros:
     - Symmetric. Both VMs go through the same `Bridge::submit` path.
     - No new dependencies; no disk concerns.
     - The property test still asserts the load-bearing claim:
       *encoding + execution paths preserve the dual-projection
       invariant* under arbitrary sequences.
   - Cons:
     - Doesn't catch bytecode-level divergence (e.g., an EVM `SELFBALANCE`
       opcode disagreeing with a `BALANCE` opcode for the same account).
       That class of bug only surfaces with real revm.
     - The "real Move VM" question is deferred, not answered.

3. **Defer S3 entirely.**
   - Pros: keeps spec intact.
   - Cons: blocks every downstream sprint that builds on the projector
     wiring (S4 OCC, S5 cross-VM intent queue, S6 Verkle).

### Decision

**Option 2** — mock executors in phase-1 S3.

The mock executors honour EVM revert and Move abort semantics, route
through the same `Bridge::submit` path, and the property tests run at
10,000 cases over interleaved EVM+Move transaction sequences. This
operationalises the dual-projection invariant at the encoding layer,
which is what S3's wiring goal actually demands.

Real-VM integration becomes a follow-up sprint, **provisionally S3.5**,
to land *before* S5 (cross-VM intent queue) — because S5's correctness
proof needs real VM semantics, not modelled ones. We keep the trait
shape (`MockEvm`, `MockMove`) so swapping in `RevmExecutor` and a
chosen-Move-VM executor is a drop-in replacement; the property test
continues to enforce the same invariant against the real backends.

> **Update (2026-05-08, per IQ-3):** S3.5 was dissolved. Real-revm
> integration folds into S5 where contract calls give it real bug-
> finding value. The Move VM dialect choice is reframed as a launch-
> readiness decision, not a phase-1 sprint deliverable. Phase-1 ships
> with `MockMove` through S8.

### Consequences

- **Spec changes:** `docs/spec/dual-vm-projectors.md` (added in this
  slice) notes the mock-executor caveat in its "Failure model" section
  and points to this IQ.
- **ADR changes:** None. ADRs aren't yet established in this repo.
- **Code changes:**
  - `crates/gsxdb-bridge/src/vm/executor.rs` ships `MockEvm` and
    `MockMove`. They are public; lane code can call them. (Lane code
    already had to go through the bridge anyway, so this preserves the
    lane-separation invariant.)
- **Test changes:** `cross_vm_parity` tests run against mock executors.
  When real backends land, the same tests run against `RevmExecutor` and
  `<chosen-move-vm>Executor` with no test changes.

### What still needs an IQ

- **IQ-3 candidate: which Move VM crate.** Aptos vs Sui vs writing a
  minimal Move interpreter. Each has different tradeoffs for binary
  size, framework lock-in, and on-chain compatibility. Should be
  resolved before S3.5 starts.
- **Address shape.** Phase-1 uses 20-byte EVM-style addresses
  everywhere; Move (Aptos) is 32-byte. When real Move VM lands we need
  to decide: pad EVM addresses to 32 bytes globally, or maintain a
  20↔32 byte mapping at the projector layer. Probably IQ-4.
- **Nonces.** EVM has them as part of replay protection; Move's
  signing model doesn't use them in the same way. Phase-1 mocks ignore
  nonces; real revm will force the question. Probably IQ-5.

### Propagation checklist

- [x] Code: `MockEvm`, `MockMove` ship in `gsxdb-bridge::vm::executor`
- [x] Tests: `cross_vm_parity` proptest at 10k cases passing
- [x] Doc: `docs/spec/dual-vm-projectors.md` written and references this IQ
- [ ] S3.5: introduce `RevmExecutor` swapping in real EVM bytecode
- [ ] S3.5 / IQ-3: pick a Move VM crate (or write minimal interpreter)
- [ ] IQ-4 (later): resolve address-shape mismatch
- [ ] IQ-5 (later): resolve nonce semantics
