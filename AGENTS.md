# AGENTS.md — gsx-db

Guidance for AI agents (Codex PR review, Claude, etc.) working in this repo.
Codex reads the nearest `AGENTS.md` when reviewing a PR; this front-loads the
load-bearing invariants so the highest-risk changes are flagged before a human
reviewer (Steve) looks.

## What this repo is
GSX-DB is the storage + validation engine under the Global Settlement Network:
a dual-VM (EVM + Move) state database over a polymorphic balance map, committed
by a state tree (BLAKE3 default; Verkle/IPA under `production-verkle`), with
Rust↔Solidity anchor parity. It backs the gsx-dag Mysticeti L1.

## P0 — flag ANY change touching these as P0 in review

A defect here is **unrecoverable / forks the chain**. Treat a diff to these
surfaces as P0, demand a test that pins the invariant, and call out anything
that changes committed bytes:

1. **State-root / commitment determinism.** `crates/gsxdb-state/src/tree/`
   (`commit.rs` BLAKE3 leaf, `verkle_scheme.rs` IPA leaf), `snapshot.rs`. Both
   commitment backends MUST commit the same logical fields (balance **and**
   nonce, plus EVM code/storage/bytes_state where in scope). A change that adds
   a field to one backend but not the other, or changes the leaf encoding
   without bumping the snapshot magic + updating recovery/migration, is P0.
   Any change to what bytes enter the root must be called out explicitly — it
   ripples to anchors, proofs, recovery, and the gsx-dag consumer (#269).
2. **Proposition 1 — dual-VM consistency.** At every checkpoint
   `EVM balanceOf(addr) == Move Coin.value(addr)` (and nonce/sequence agree).
   Enforced by the 10k parity proptest. Any executor/projector/balance-slot
   change must keep it green; flag if the proptest isn't run.
3. **Lane separation.** `gsxdb-lane` (untrusted ingest) must NOT mutate
   `gsxdb-state` directly — all mutations flow through `gsxdb-bridge` behind the
   `BridgeToken` capability gate. A new `pub` raw-mutation API (set balance /
   nonce / code / storage / bytes) that bypasses `Bridge::submit` validation is
   P0 — it must be `pub(crate)` or capability-gated. (`check-lane-separation.sh`
   + `deny.toml` enforce the crate-dep direction.)
4. **Nonce / replay protection.** EVM nonce write-back must advance the
   canonical slot AND be reflected in the committed root + snapshot. A path that
   resets a nonce to 0 (e.g. `SetBalance` clobbering an advanced nonce), or
   validates an envelope nonce against the wrong source, re-enables transaction
   replay — P0.
5. **Anchor parity (Rust ↔ Solidity).** `crates/gsxdb-bridge/src/anchor/` and
   `contracts/` must accept/reject the same inputs for all entity-state-machine
   pairs. Any FSM / record-layout / hash-domain change must update BOTH sides +
   the differential test, or it's P0.
6. **Recovery / snapshot migration.** A snapshot format bump (magic version)
   needs a decode path or explicit rejection for the prior version, and
   replaying pre-upgrade blocks must still reproduce the committed root. Missing
   migration / replay-compat is P0.
7. **Crypto surfaces.** Verkle/IPA, ML-DSA/PQ, key custody, BLS/secp signing.
   Constant-time where it matters; no silent primitive swaps.

## Conventions (hard rules)
- **No `git rebase`** on shared branches; use `git merge` / `git pull --no-rebase`.
- **No `Co-Authored-By`** trailers. Sign off commits (`-s`) — DCO is enforced.
- **No `--no-verify`** / hook bypass.
- Property tests: **≥10k iterations** for invariants. Conformance fixtures in
  `tests/parity-fixtures/` are shared with Solidity — keep them in sync.
- Feature gates: `production-move-executor`, `production-verkle`,
  `production-evm-executor` are off by default; CI exercises them in dedicated
  jobs. Don't make a private/heavy dep part of *default* resolution.

## Review tiering (see `.github/CODEOWNERS`)
- **Tier-1** (consensus/crypto/parity/recovery paths) → requires a non-author
  maintainer (Steve). Agents should be most rigorous here.
- **Tier-2** (everything else) → CI + this agent review + one LGTM. Keep PRs
  small (200–400 LOC) and reviewable as standalone layers.

## Verify a change
```bash
make test-python && make test-contracts          # parity surfaces
cargo test --workspace                            # default backend
cargo test -p gsxdb-state --features production-verkle      # verkle root
cargo test -p gsxdb-bridge --features production-evm-executor # real EVM
```
