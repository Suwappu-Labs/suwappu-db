# Engineering standards for enterprise + academic rigor

This document defines non-negotiable standards for evolving Suwappu DB as a
production database engine with research-grade correctness evidence.

## 1) Repository structure contract

Top-level responsibilities are fixed:

- `crates/suwappudb-state` — authoritative data model, storage backends, commitment tree.
- `crates/suwappudb-bridge` — only mutation path from intents to state transitions.
- `crates/suwappudb-lane` — ingestion and ordering; no direct state mutation.
- `docs/spec` — normative behavior specs (must match code semantics).
- `docs/architecture` — design rationale, tradeoffs, sprint maps, IQ backlog.
- `docs/iq` — explicit decision records with alternatives and consequences.
- `scripts/` — reproducible local/CI checks and operator workflows.

### Boundary rules

1. Cross-crate dependency flow is one-way unless explicitly approved:
   - `lane -> bridge -> state`
   - never `lane -> state`
2. New module placement must follow ownership:
   - consensus/execution semantics in `bridge`
   - data durability/indexing in `state`
   - ingestion/policy in `lane`
3. Every new subsystem requires:
   - a normative spec page in `docs/spec/`
   - at least one property/invariant test
   - an IQ update if it changes a major design decision

## 2) Code quality gates (enterprise baseline)

Every PR should pass:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
scripts/check-lane-separation.sh
```

Additional gate for execution/storage changes:

```bash
cargo test -p suwappudb-bridge
cargo test -p suwappudb-state
```

### Required PR checklist

- [ ] API surface documented (`///` docs on public items)
- [ ] Error behavior explicit and typed (no silent panic paths on untrusted data)
- [ ] Determinism preserved (no nondeterministic ordering in execution/commit paths)
- [ ] Backward compatibility assessed for persisted data formats
- [ ] Spec/docs updated in same PR when semantics change

## 3) Verification standards (academic baseline)

### Invariant-first development

Any functional feature must declare the invariant it preserves before coding.

Template:

- **Invariant:** precise statement
- **Scope:** which crate/module boundaries it spans
- **Adversary model:** malformed input, replay, corruption, concurrency, etc.
- **Evidence:** unit/integration/property tests + any differential harness

### Property testing requirements

- New invariants use `proptest` with explicit rationale for case count.
- Exit-gate properties should target >= 10k cases unless runtime makes this
  impractical; any lower count must be justified in PR notes.
- Seeds/regressions are checked in when a failing case is found.

### Differential/conformance testing

When an external implementation exists (Solidity contract, reference cryptography,
VM runtime), add cross-implementation parity checks and fixtures.

## 4) Persistence and schema evolution discipline

- Persisted byte formats must be versioned or strictly documented as fixed.
- Decoders must be defensive: malformed bytes should return typed errors/None,
  never trigger process panics.
- Migration strategy required before changing persistent layout:
  - read-old/write-new compatibility plan, or
  - explicit one-time migration tool + rollback plan.

## 5) Documentation rigor

- `docs/spec/*` are normative: if code disagrees, code is wrong or spec must be
  updated in the same PR.
- `docs/architecture/*` are explanatory and must link to specs/tests.
- Placeholder/deferred statements must include an owner sprint or IQ ID.

## 6) Release-readiness scorecard

A sprint is not complete until all are true:

1. Exit-gate property test green at target case count.
2. Workspace tests green.
3. Specs updated and reviewed.
4. Open risks + deferred items documented with owners.
5. Recovery path tested (restart/replay where relevant).

## 7) Immediate restructuring roadmap

1. **Normalize quality gates in CI**
   - enforce clippy `-D warnings` and lane-separation script in default pipeline.
2. **Create audit ledger from placeholder plan**
   - instantiate `docs/architecture/audit-ledger.md` and track every placeholder line.
3. **Formalize persisted format docs**
   - add explicit encoding/version sections for state/recovery stores.
4. **Expand differential testing**
   - wire parity harnesses for anchor/VM/commitment swaps.
5. **Adopt change-control templates**
   - PR template with invariants, adversary model, evidence, and migration impact.

This is the minimum bar for enterprise operations and academic credibility.
