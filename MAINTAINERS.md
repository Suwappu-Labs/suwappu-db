# Maintainers

## Current

| Handle | Role | Coverage |
|---|---|---|
| [@tomasuwappu](https://github.com/tomasuwappu) | Lead maintainer | All surfaces. Security disclosure contact. |

## Areas + suggested reviewers

When opening a PR, the [`.github/CODEOWNERS`](./.github/CODEOWNERS)
file auto-requests reviewers per path. The table below is the
human-readable version of the same map.

| Area | Reviewer hint |
|---|---|
| Lane separation (`suwappudb-lane`, `suwappudb-bridge`, `scripts/check-lane-separation.sh`, `deny.toml`) | @tomasuwappu — invariant guard |
| Cryptography (`suwappudb-state/tree/`, `signing.rs`, `credential.rs`) | @tomasuwappu — correctness + side-channels |
| Anchor parity (`anchor/`, `LTPAnchorRegistry.sol`, `contracts/`) | @tomasuwappu — Rust ↔ Solidity parity |
| Recovery + DAG (`recovery/`, `dag.rs`, `snapshot.rs`) | @tomasuwappu — replay soundness |
| Security policy + audit ledgers (`SECURITY.md`, `docs/audit/`) | @tomasuwappu |
| CI workflows (`.github/`) | @tomasuwappu |

## Becoming a maintainer

Maintainership is invitation-based, granted by existing maintainers
to contributors with a sustained record of merged, reviewed work in
the areas above. We follow the same path as Foundry / Tempo —
quality of contributions over volume; emphasis on architectural
judgement; comfort with the IQ + audit-ledger governance pattern
(see [`GOVERNANCE.md`](./GOVERNANCE.md)).

If you'd like to be considered, the practical path is:

1. Land a few PRs that touch one of the surfaces above.
2. Review others' PRs (you can do this without being a maintainer).
3. After ~3-6 substantive contributions, an existing maintainer
   opens an issue proposing you for the role. Other maintainers
   weigh in; lazy-consensus 7-day window.

## Reaching the maintainers

- **General questions / proposals:** open a GitHub issue or
  Discussion.
- **Security vulnerabilities:** [`SECURITY.md`](./SECURITY.md) —
  private disclosure only.
- **Anything else:** open an issue and tag a maintainer.
