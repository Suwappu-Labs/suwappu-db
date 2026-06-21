# IQ-3: Move VM choice — defer, then Aptos

**Status:** Accepted (revised in S9 — Aptos selected)
**Original date:** 2026-04 (phase-1 deferral)
**Revised date:** 2026-05-09 (S9 decision)
**Sprint context:** phase-1 (S5 dissolved S3.5 → IQ-3) → S9 (launch readiness)

> Consolidates the phase-1 deferral and the S9 selection. Phase-1
> reasoning preserved for context; S9 decision is binding.

```mermaid
flowchart LR
    P1[Phase-1<br/>Mock Move VM]
    IQ3[IQ-3 deferral]
    Eval{S9 evaluation<br/>of 5 options}
    Apt[Aptos<br/>move-vm-runtime]
    Sui[Sui fork]
    Up[Upstream move-language]
    HR[Hand-rolled interpreter]
    Def[Defer further]

    P1 --> IQ3
    IQ3 --> Eval
    Eval --> Apt
    Eval -.x.-> Sui
    Eval -.x.-> Up
    Eval -.x.-> HR
    Eval -.x.-> Def
    Apt --> S9[S9: AptosVM<br/>integration]
    style Apt fill:#cfc
    style S9 fill:#cfc
```

---

## Part 1 — Phase-1 deferral (original)

### Question

S3 originally planned to integrate a real Move VM in S3.5. Two
sub-questions: (a) which Move VM dialect; (b) when to integrate.
Dialect options:

1. Aptos `move-vm-runtime`
2. Sui's vendored fork
3. Upstream `move-language/move`
4. Hand-rolled minimal interpreter
5. Defer the question

### Phase-1 decision

**Option 5 — defer.** Phase-1 shipped `MockMove` validating only the
structural invariants (dual-projection, OCC determinism, cross-VM
canonical equivalence). Real Move integration was folded out of
S3.5 into a launch-readiness item.

### Rationale

- 10k-case property tests verify load-bearing invariants without
  bytecode execution semantics.
- Each candidate VM has different ergonomics (Aptos: `Coin<T>`
  framework; Sui: object-centric; upstream: minimal). Choice
  cascades into IQ-4 (address shape) and IQ-5 (nonces).
- The trait surface (`MoveProjector`) is dialect-independent.

### What this left open

- The dialect choice itself (resolved below).
- S9 wiring of real bytecode.
- Address shape (IQ-4) and nonce semantics (IQ-5) cascade from the
  dialect choice.

---

## Part 2 — S9 revision: Aptos selected

### Decision (binding)

**Aptos `move-vm-runtime` as the primary integration.**

### Comparison

| Criterion | Aptos | Sui fork | Upstream | Hand-rolled |
|---|---|---|---|---|
| Mainnet throughput proven | ✅ | partial | ❌ | ❌ |
| Ecosystem (TVL, devs) | largest | second | small | none |
| crates.io publication | ✅ | ❌ | ❌ | ✅ |
| Standard token model | `Coin<T>` | `Object<>` | bare | ours |
| Build size | ~500 MB | ~700 MB | ~200 MB | <1 MB |
| Security patch ownership | Aptos Foundation | Mysten Labs | community | us |
| Retreat path if untenable | hand-rolled | Aptos | Aptos | Aptos |

### Trade-offs (accepted)

- **Framework lock-in.** Suwappu-DB Move code will be Aptos-flavoured
  (ability sets, gas model, native function ABIs). Acceptable —
  Aptos is the launch ecosystem.
- **Build size.** ~500 MB target dir. Mitigated by Docker layer
  caching in CI.
- **Upgrade cadence.** We track Aptos releases and own the
  downstream security posture.

### Implementation surface (S9)

- Import `aptos-core` (mainnet-compatible, ~0.7.x stable)
- Replace `MockMove` with `aptos_vm::AptosVM`
- Adapt `MoveProjector` to use AptosVM syscalls
- Property tests re-run under real bytecode to confirm
  Proposition 1 (dual-projection) holds

### Consequences

- `suwappudb-bridge` gains `move-vm` feature gate (off by default in
  fast tests; on for the launch-readiness exit gate)
- `Cargo.toml` pins `aptos-core` version
- `MoveProjector::execute` calls real bytecode behind the feature
- IQ-4 (address shape — 20B vs 32B) now resolvable
- IQ-5 (nonce semantics — sequence numbers per Aptos model) now
  resolvable

### What's still open

- **IQ-4 (address shape).** EVM 20-byte vs Aptos 32-byte. Resolved
  via `Address` enum + canonical projection.
- **IQ-5 (nonce semantics).** Resolved via per-account
  `AccountNonce { evm: u64, move_seq: u64 }`.
- **Snapshot checkpoints (IQ-9).** Deferred to S12.

### Propagation checklist

- [x] `crates/suwappudb-state/src/vm/executor.rs` — placeholder trait + Mock impl
- [x] `production-move-executor` feature gate (empty — pulls no deps yet)
- [ ] **Trait redesign** — current trait `(addr, BalanceSlot) → ExecutionOutcome` is a passthrough; not invoked anywhere. Replace with `(MoveCall, &dyn ModuleStore, &mut MoveSessionState) → MoveOutcome`. See `docs/spec/move-execution.md`.
- [ ] `ModuleStore` trait + in-memory + redb-backed impls
- [ ] `Intent::DeployModule` + `Intent::Call` Move-arm wiring through `BundleExecutor`
- [ ] `aptos_vm::AptosVM` (actually: `move-vm-runtime` subset) wired behind the feature
- [ ] Canonical suwappu-db `Coin<T>` Move module bundled
- [ ] Re-run dual-projection 10k proptest with real bytecode
- [x] `docs/spec/move-execution.md` ✅ — S9.1 (this PR)
- [x] IQ-4 (address shape) resolved in spec — `Address` enum with canonical projection
- [x] IQ-5 (nonce semantics) resolved in spec — per-account `AccountNonce { evm, move_seq }`

### S9 sub-pass breakdown

| Sub-pass | What | Status |
|---|---|---|
| S9.1 | Design doc (`docs/spec/move-execution.md`) + IQ-3 update | ✅ landed |
| S9.2 | New trait surface in `suwappudb-state::vm` + new `MockMoveExecutor` | ✅ landed |
| S9.3 | `InMemoryModuleStore` + `Intent::DeployModule` wire format | ✅ landed |
| S9.4 | `BundleExecutor::execute_with_move_runtime` (MoveCall + DeployModule with deferred-commit) | ✅ landed |
| S9.5a | `AptosMoveExecutor` scaffold + dep-choice docs | ✅ landed |
| S9.5b | Pull `aptos-core` git deps, cargo-deny update (2 advisory ignores + 2 git sources, no license issues) | ✅ landed |
| S9.5c | Real `CompiledModule::deserialize` + `verify_module` in `AptosMoveExecutor` | ✅ landed |
| S9.5d | Session-layer design doc (`docs/spec/move-vm-session-layer.md`); inventories the ~60 trait methods + 4 open questions | ✅ landed |
| S9.5e | **Build** the session layer (`Loader` + `ModuleStorage` + `MoveVmDataCache` + `GasMeter`) | pending — multi-session per design doc |
| S9.5f | Compile + bundle canonical suwappu-db `Coin<T>` Move module | pending |
| S9.6 | Flip `production-move-executor` ON by default + 10k cross-VM parity gate with real bytecode | pending |

S9.5 had to split deeper than expected. The Aptos `MoveVM` at tag
`aptos-node-v1.44.9-hotfix` is stateless and requires the caller to
provide ~60 methods worth of session machinery normally supplied by
`aptos-vm` (which we deliberately don't pull). See
`docs/spec/move-vm-session-layer.md` for the inventory + plan.
