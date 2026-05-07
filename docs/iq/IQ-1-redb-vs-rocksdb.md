## IQ-1: Phase-1 storage backend — redb in development, RocksDB in production

**Status:** Accepted
**Date:** 2026-05-07
**Sprint context:** S2 slice 3 (PBM persistent backend)

### Question

The Phase-1 spec lists RocksDB as the canonical storage backend for the
state lane (`state`, `aggregates`, `evm_storage`, `evm_nonces`,
`move_resources` column families). RocksDB is the right choice for
production: it's the most battle-tested embedded KV in this category, has
column families that map naturally to PBM's CFs, and is the operational
default for every blockchain node we'd compare against.

But the local development environment cannot build it. The `rocksdb-sys`
crate compiles a 600+ MB C++ library, and the developer machine running
this work has 1.1 GiB of free disk — exhausted twice during the static-link
step. Freeing more space requires user-managed cleanup of personal files.

### Context

S2's exit gate is the dual-projection invariant property test passing
against persistent storage. Slices 1 and 2 satisfy the type-level and
in-memory-storage forms of the invariant; slice 3 must do the persistent-
storage form to close the sprint.

The `BalanceStore` trait was deliberately introduced in slice 2 as an
abstraction layer so the persistent backend can swap without changing
callers. That bet pays off here.

### Options considered

1. **Free local disk and use RocksDB.** Spec-compliant. Requires manual
   cleanup of ~3 GB of personal files on the dev machine. Blocks on the
   developer.
   - Pros: matches spec, single backend across dev and prod.
   - Cons: not actionable in this session; future re-occurrence likely on
     other dev machines.

2. **Switch the backend to redb (pure-Rust embedded KV).** Builds in
   seconds, no native deps, ~10 MB compiled artifact. Implements the
   `BalanceStore` trait with the same semantics. Production deploys still
   pin RocksDB in a Docker image where disk isn't a constraint.
   - Pros: unblocks slice 3 today; trait abstraction means production
     RocksDB swap is mechanical when prod build is wired up.
   - Cons: divergence between dev and prod backends; redb's snapshot
     isolation differs from RocksDB's (single-writer vs multi-writer);
     phase-2 may need to re-validate persistence semantics under
     concurrent CE-MVCC writers.

3. **Defer slice 3 to a dedicated session.** Close S2 at slice-2 (in-memory
   exit gate met but not the persistent-storage one). Move on to S3.
   - Pros: keeps spec intact.
   - Cons: persistent dual-projection invariant remains unverified until
     someone resolves the local disk situation; S3 builds on storage
     assumptions that we'd then test in arrears.

### Decision

**Option 2.** Adopt redb as the development backend behind a
`RedbBalanceStore` impl of `BalanceStore`. RocksDB remains the production
backend — we will introduce `RocksDbBalanceStore` in S8 (DAG store +
recovery) when the production deploy is being wired and the build runs in
a Docker image with adequate disk.

The trait abstraction means both impls coexist; choice is made at `State`
construction time. Dev-mode builds default to redb; prod-mode builds use a
feature flag (`production-storage`) to gate the RocksDB impl in.

### Consequences

- **Spec changes:** `docs/spec/storage.md` (when written) must note both
  backends and that redb satisfies the same `BalanceStore` contract that
  RocksDB will. The 5-CF design becomes 5-table in redb terms — same
  cardinality, same names.
- **ADR changes:** None yet (no ADR exists; this IQ stands alone).
- **Code changes:**
  - `crates/gsxdb-state/Cargo.toml`: replace `rocksdb` with `redb`
  - `crates/gsxdb-state/src/redb_store.rs`: new module
  - `crates/gsxdb-state/src/lib.rs`: re-export `RedbBalanceStore`
  - `crates/gsxdb-state/src/rocks_store.rs` (uncommitted): delete
- **Test changes:** The existing slice-3 property tests
  (`rocks_preserves_dual_projection`, `rocks_matches_in_memory`) become
  `redb_*` equivalents and run against the new backend.

### Propagation checklist

- [x] Code: replace `rocksdb` dep with `redb` in `crates/gsxdb-state/Cargo.toml`
- [x] Code: implement `RedbBalanceStore` with same 5-table layout
- [x] Code: re-export from `lib.rs`
- [x] Tests: slice-3 property tests against redb backend
- [x] `.sprint-state.md`: note IQ-1 in slice-3 entry
- [ ] `docs/spec/storage.md`: update when written (spec doc not yet authored)
- [ ] S8: introduce `RocksDbBalanceStore` behind `production-storage` feature
