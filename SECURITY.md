# Security policy

## Reporting a vulnerability

**Do not file public issues for security vulnerabilities.** Use the
private disclosure path so we can coordinate patches and credit.

### Disclosure channels

In order of preference:

1. **GitHub Security Advisory** (private) — use the "Report a
   vulnerability" link on the repo's Security tab. This creates
   a private advisory only project maintainers can see.
2. **Email** — `security@globalsettlement.com`. PGP key fingerprint
   is documented at https://www.globalsettlement.com/security
   (when published). Until then, use the GitHub channel.

Include:

- A description of the vulnerability and its blast radius.
- Steps to reproduce (PoC code if applicable).
- Suggested mitigation if you have one.

### Response timeline

| Phase | Target |
|---|---|
| Initial acknowledgement | Within 48 hours |
| Triage + severity rating | Within 5 business days |
| Patch ready in private | Severity-dependent — critical ≤ 14 days, high ≤ 30, medium ≤ 60 |
| Public disclosure | Coordinated with reporter; default 90-day embargo, extensions on request |

### Scope

In-scope:

- suwappu-db itself (this repository).
- The `LTPAnchorRegistry` Solidity contract and its parity model.
- The published JSON-RPC surface (`/v1/rpc`).
- Any binary or container image published under
  `Suwappu-Labs/suwappu-db` GitHub Releases or ECR Public.

Out of scope (track upstream):

- `aptos-core` (Move VM), `crate-crypto/rust-verkle`,
  `crate-crypto/banderwagon` — report to those projects directly.
- `suwappu-dag`, `suwappu-lattice-protocol` — sibling repos with their own
  `SECURITY.md`.
- Third-party deployments that customise the stack — work with
  the operator.

### Safe harbour

We will not pursue legal action against good-faith security
researchers who:

- Provide a reasonable disclosure window before going public.
- Avoid privacy violations, destruction of data, and disruption
  of services.
- Operate within the scope above.

## Supported versions

| Version | Security updates |
|---|---|
| `0.1.0-pre` (current) | ✅ active development |
| `< 0.1.0` (substrate-only) | not supported — superseded by 0.1.0-pre Phase-1 |

Until `v1.0.0`, only the latest minor receives patches. From
`v1.0.0` onward, the latest major + the most recent prior major
receive security backports.

## Audit ledgers

Pass B (security audit + hardening) findings + verdicts:
[`docs/audit/pass-b-2026-05-16.md`](./docs/audit/pass-b-2026-05-16.md)

Pass C (external-dev readiness) findings:
[`docs/audit/pass-c-2026-05-16.md`](./docs/audit/pass-c-2026-05-16.md)
