# Changelog

All notable changes to gsx-db are recorded here.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versioning follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
once the project is tagged. Pre-tag, every change lands under `[Unreleased]`.

## [Unreleased]

### Added

- **S2 — Persistent dual-projection invariant.**
  - `BalanceSlot` type with `EvmBalance` / `MoveCoinValue` projections.
    Proposition 1 enforced structurally: one canonical field, two views.
  - `BalanceStore` trait + `InMemoryBalanceStore` impl.
  - `RedbBalanceStore` impl with five tables (`state`, `aggregates`,
    `evm_storage`, `evm_nonces`, `move_resources`) materialised at open.
  - Property tests at three layers — type, in-memory, persistent — all
    passing the dual-projection invariant under arbitrary mutations.
  - `State` refactored to delegate to a pluggable `BalanceStore` backend.
  - End-to-end integration: `Lane → Bridge → State → redb`.
- **IQ-1.** Phase-1 storage backend split: redb in dev/CI, RocksDB in
  production (S8). Decision driven by local build-disk constraints.
- **S1 — Workspace + lane separation.**
  - Three core crates: `gsxdb-state`, `gsxdb-bridge`, `gsxdb-lane`.
  - Capability-token gate: `State::apply` requires a `BridgeToken` that
    only `gsxdb-bridge` can mint.
  - `scripts/check-lane-separation.sh` blocks `gsxdb-lane` from importing
    or depending on `gsxdb-state`. Verified to catch both `Cargo.toml`
    and source-level violations.
- **Workspace lints.** `forbid(unsafe_code)`, `deny(clippy::all)`,
  `warn(clippy::pedantic)` across all crates.
- **Project scaffolding.** `CLAUDE.md`, slash commands (`/sprint`,
  `/check`, `/release`, `/aws-status`, `/audit-bridge`, `/cross-parity`,
  `/iq-decision`), subagents (`lane-auditor`, `crypto-reviewer`,
  `parity-checker`, `sprint-runner`), `claude-code/install.sh`.

### Changed

- _nothing yet_

### Fixed

- _nothing yet_

[Unreleased]: https://github.com/GlobalSettlementNetwork/gsx-db/commits/main
