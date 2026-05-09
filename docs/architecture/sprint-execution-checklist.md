# Sprint execution checklist (S8.5–S12)

This is the standards-driven execution tracker for the remaining roadmap.
Each sprint must satisfy `engineering-standards.md` quality and evidence gates.

## Global gates (apply to every sprint)

- [ ] Invariant declared before implementation
- [ ] Adversary model documented
- [ ] Property/integration test evidence added
- [ ] Spec updated in same PR as behavior changes
- [ ] Persistence/migration impact reviewed
- [ ] `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green

## S8.5 — Redb block-store durability

- [x] redb-backed `RedbBlockStore` implemented
- [x] restart round-trip test exists
- [x] decoder hardened against truncated/corrupt payloads
- [x] explicit encoding version byte added
- [ ] persistence format section added to `docs/spec/recovery.md`
- [ ] crash/partial-write fault-injection tests added

## S9 — Real Move VM + address/nonce semantics

- [ ] Move VM dialect decision finalized (IQ update)
- [ ] VM adapter boundary for mock/real swap
- [ ] address shape normalization rules implemented + tested
- [ ] nonce semantics implemented + tested through OCC
- [ ] cross-VM parity suite green with real Move path in matrix

## S10 — Real Verkle + IPA witnesses

- [ ] commitment backend swap behind stable tree API
- [ ] witness generation + verification implemented
- [ ] differential parity harness against reference impl
- [ ] scale/perf evidence captured
- [ ] proof/commitment serialization versioned

## S11 — Solidity registry + signatures parity

- [ ] Solidity `LTPAnchorRegistry` FSM parity complete
- [ ] signature domain model finalized + tested
- [ ] shared parity fixtures between Rust/Solidity
- [ ] 36-pair matrix green in CI

## S12 — DAG + snapshots + telemetry + shadow

- [ ] DAG data model and replay order policy implemented
- [ ] snapshot/checkpoint mechanism implemented
- [ ] telemetry for replay/latency/conflict metrics added
- [ ] shadow testnet comparison and SLO gates validated
