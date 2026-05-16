# Governance

How decisions get made in gsx-db. Three layers, matching the
artefacts already in the repo.

## Tl;dr

| Decision type | Mechanism | Artefact |
|---|---|---|
| **Day-to-day code** | PR review by CODEOWNERS | GitHub PR + DCO sign-off |
| **Design decisions** | Investigation Question (IQ) record, agreed before code lands | [`docs/iq/IQ-N-<topic>.md`](./docs/iq/) |
| **Security / risk** | Audit-ledger entry, verdict signed by maintainer | [`docs/audit/pass-*.md`](./docs/audit/) |
| **Breaking changes** | CHANGELOG entry, version bump, INTEGRATORS stability note | [`CHANGELOG.md`](./CHANGELOG.md) + [`INTEGRATORS.md`](./INTEGRATORS.md) |

## Day-to-day code changes

Standard PR workflow per [`CONTRIBUTING.md`](./CONTRIBUTING.md):

1. Branch from `main` with `<scope>/<short-slug>`.
2. Sign commits with `git commit -s` (DCO; checked by CI).
3. CI must be green: `clippy --workspace -- -D warnings`,
   `cargo-deny`, `cargo-audit`, gitleaks, 10k proptest gate.
4. At least one approving review from a CODEOWNER.
5. Squash or merge; no force-push to main.

**No `git rebase`** — repo convention; preserves audit-ledger
traceability.

## Design decisions — Investigation Questions

Non-trivial design calls (storage backend, VM choice, commitment
scheme, anchor protocol, …) get recorded as an **IQ** before code
lands. Format documented in [`docs/iq/`](./docs/iq/); examples:

- [IQ-3](./docs/iq/IQ-3-move-vm-choice.md) — Move VM dialect
- [IQ-6](./docs/iq/IQ-6-verkle-commitment.md) — Verkle vs BLAKE3
- [IQ-7](./docs/iq/IQ-7-anchor-parity.md) — anchor verifier

Process:

1. Open a GitHub issue tagged `iq/proposed`.
2. Draft an IQ document under `docs/iq/IQ-N-<short-slug>.md`.
3. Discuss in the issue or in a draft PR.
4. Lazy consensus: 7 days, ≥ 1 maintainer +1 explicit, no
   maintainer block → accepted. Otherwise revisit.
5. Mark the IQ `Accepted` + cite it from the implementing
   sprint / PR commits.

**Why this layer exists:** code review is the wrong stage to argue
about architecture. An accepted IQ shortcuts the inevitable
"why did we pick X over Y" discussion that re-surfaces every six
months.

## Security and risk — audit ledgers

For passes that touch security-critical surfaces (anchor
verification, key custody, network endpoints, panic surfaces), we
maintain audit ledgers under [`docs/audit/`](./docs/audit/):

- Each engagement opens a ledger with the date + scope.
- Each finding gets a verdict: ✅ Met / ⚠ Accepted / ❌ Blocked.
- Verdict is signed by the reviewing maintainer in the commit.
- Closed ledgers stay in-tree forever — they document **why** we
  accepted (or didn't) each known divergence.

Precedent: [Pass B](./docs/audit/pass-b-2026-05-16.md) (security
audit + hardening), [Pass C](./docs/audit/pass-c-2026-05-16.md)
(external-dev readiness). Both shipped with the `v0.1.0-pre` tag.

## Breaking changes

Pre-1.0, minor bumps **may** include breaking changes. We require:

1. A `## [VERSION]` section in [`CHANGELOG.md`](./CHANGELOG.md)
   that names the break under `### Changed` or `### Removed`.
2. An updated entry in
   [INTEGRATORS.md "Stability promises"](./INTEGRATORS.md#stability-promises)
   if the affected surface was previously marked frozen.
3. A 1-cycle deprecation alias where feasible (see the `/rpc` →
   `/v1/rpc` migration in C4 for the canonical pattern).

From `v1.0.0` onward, strict SemVer governs. Breaking changes
require a major bump + at least one minor-version deprecation
window.

## Disputes + escalation

If a CODEOWNER review and a contributor disagree past one cycle
of back-and-forth:

1. Open or reopen the originating issue / IQ.
2. Tag all maintainers explicitly.
3. Maintainers vote with explicit comments: `+1`, `-1`, or
   `+0` (no opinion). Simple majority of non-`+0` votes decides.
4. In case of tie, the lead maintainer (currently @tomagsx)
   breaks it. The tiebreaker reason is recorded in the
   IQ / audit ledger.

## Maintainer veto

A maintainer may veto:

- Security-sensitive surfaces (audit ledgers' ⚠ Accepted findings).
- Breaking changes to frozen types listed in
  INTEGRATORS.md "Stability promises".
- Repo conventions explicitly named in [`CLAUDE.md`](./CLAUDE.md)
  (no rebase, no `Co-Authored-By` lines, etc.).

A veto is recorded with reasoning; the contributor may re-open the
discussion with new context.

## Process changes

Changes to this document (`GOVERNANCE.md`) require:

1. Issue or PR proposing the change.
2. Lazy consensus from all current maintainers (7 days).
3. Update [`MAINTAINERS.md`](./MAINTAINERS.md) if the change
   affects the maintainer list.
