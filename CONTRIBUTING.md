# Contributing to suwappu-db

Thanks for your interest. This repo is open to external contributions
under the rules below. For integrator-facing usage (depending on
suwappu-db rather than changing it), see [`INTEGRATORS.md`](./INTEGRATORS.md).

## TL;DR

1. **Sign your commits with `git commit -s`** (Developer Certificate
   of Origin — see below).
2. **Branch from `main`** using `<scope>/<short-slug>` naming
   (`anchor/foo-bar`, `tree/baz-qux`, `hardening/x`, `external-dev/y`).
3. **Write a focused PR** that addresses one thing. Reference the
   IQ or sprint it relates to in the body.
4. **CI must pass** — workspace tests, clippy strict, cargo-deny,
   cargo-audit, secret-scan, the 10k property tests, and the
   cross-impl parity gate. PRs with red CI don't get reviewed.
5. **Wait for at least one approving review** from a CODEOWNER
   before merging. CODEOWNERS lives in `.github/CODEOWNERS`.

## DCO (Developer Certificate of Origin)

Every commit must carry a `Signed-off-by:` trailer that matches your
git author email. The DCO is the lightweight alternative to a CLA:
you assert that you have the right to contribute the patch under the
project's Apache-2.0 license. The full text is at
[developercertificate.org](https://developercertificate.org).

`git commit -s` adds the trailer automatically. Example:

```
Add a new field to AnchorRecord for forensic mode

Signed-off-by: Jane Contributor <jane@example.com>
```

The CI DCO check (`.github/workflows/dco.yml`) rejects PRs that
contain unsigned commits. If you forget, use:

```sh
git commit --amend -s     # last commit only
git rebase -i HEAD~N --exec 'git commit --amend --no-edit -s'   # last N commits
```

## Prerequisites

- **Rust 1.88** — pinned via workspace `rust-version` in
  `Cargo.toml`. Older / newer toolchains may compile but aren't
  CI-tested.
- **Foundry** (forge) for any change touching `contracts/`.
- **redb**, **blake3** — picked up automatically by Cargo.

## Dev setup

```sh
git clone https://github.com/suwappu/suwappu-db
cd suwappu-db
cargo test --workspace             # ~256 cases per proptest
PROPTEST_CASES=10000 \
    cargo test --workspace --release    # exit-gate (slow)
forge test --root contracts          # Solidity tests
cargo clippy --workspace -- -D warnings
cargo deny check
```

## Branch naming

| Scope | Pattern | Example |
|---|---|---|
| Sprint sub-pass | `<area>/s<N.M>-<slug>` | `verkle/s10.3-ipa-witness-generation` |
| IQ / decision | `iq/<short-slug>` | `iq/anchor-parity` |
| Security hardening (Pass B) | `hardening/b<N>-<slug>` | `hardening/b6-rpc-auth-bearer` |
| External-dev (Pass C) | `external-dev/c<N>-<slug>` | `external-dev/c5-suwappudb-types` |
| Bug fix | `fix/<short-slug>` | `fix/snapshot-clock-skew` |
| Documentation only | `docs/<short-slug>` | `docs/clarify-eth-payload` |

## Commits

- **Imperative mood** (`Add X` not `Added X`).
- **Subject line ≤ 70 chars.** Detailed context in the body.
- **No `Co-Authored-By` lines.** Repo convention (see CLAUDE.md).
- **No `git rebase`.** Use `git merge` or `git pull --no-rebase`.
  Forced histories and reordered commits make audit ledgers
  ambiguous.

## Pull requests

- **One scope per PR.** Sprint sub-passes split into smaller PRs;
  hardening fixes get their own PR per surface.
- **Body must include:**
  - Goal (1-2 sentences).
  - Surfaces touched + their disposition.
  - Tests added or updated.
  - Cross-references: the IQ / sprint / audit-ledger entry this
    addresses.
- **Don't auto-merge.** A CODEOWNER reviews and merges; auto-merge
  bypasses the audit-ledger update step.
- **Sprint sub-passes** target the canonical workflow:
  branch → push → review → merge → update the relevant ledger
  (`docs/audit/pass-*.md` or sprint table in `CLAUDE.md`).

## Tests

- **Unit tests** inline (`#[cfg(test)] mod tests`).
- **Integration tests** in `tests/`.
- **Property tests** use `proptest` — minimum **10,000 iterations**
  for invariants. Default `cases: 256` for PR runs;
  `PROPTEST_CASES=10000` for the exit-gate run that CI executes
  on push.
- **Conformance fixtures** live in
  `crates/suwappudb-bridge/tests/cross_parity.rs` and
  `contracts/test/fixtures/`. The Rust ↔ Solidity differential
  test (`LTPAnchorRegistryParityTest`) verifies every committed
  vector recovers correctly.

## Lint posture

The workspace enforces:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }
pedantic = { level = "warn", priority = -1 }
```

`unsafe { }` is a compile error in production code. Tests must
also avoid `unsafe` — env-var setters are unsafe under Rust 1.86+;
use builder constructors (`BearerAuthConfig::with_token` etc.) for
test paths instead of `set_var`.

## Specialist review (CODEOWNERS-driven)

Some surfaces require a domain reviewer:

| Surface | Reviewer hint |
|---|---|
| `suwappudb-lane`, `suwappudb-bridge`, `scripts/check-lane-separation.sh`, `deny.toml` | Lane-separation guard |
| `suwappudb-state/src/tree`, `verkle.rs`, signature paths, KEM | Cryptographic correctness |
| `anchor/`, `LTPAnchorRegistry.sol` | Rust ↔ Solidity parity |
| `recovery/`, `dag.rs`, `snapshot.rs` | Recovery + DAG soundness |

A reviewer's verdict is documented inline in the PR body and
mirrored into the relevant audit ledger if the change touches an
audited surface.

## Reporting security issues

**Do not file public issues for vulnerabilities.** See
[`SECURITY.md`](./SECURITY.md) for the private disclosure path.
We'll coordinate disclosure and credit per the standard 90-day
embargo with extensions on request.

## License

By submitting a contribution you agree to license it under
[Apache-2.0](./LICENSE). The DCO sign-off is the formal
attestation of this agreement; CLA is not required.
