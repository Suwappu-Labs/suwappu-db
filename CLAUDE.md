# GSX-DB — Claude Code project context

This file is loaded automatically at the start of every Claude Code session in this repo. It is the entry point for orienting Claude Code on conventions, current sprint state, load-bearing invariants, and how to collaborate.

## Project

GSX-DB is the storage and validation engine underneath the Global Settlement Network. Phase 1 lands the dual-VM database (EVM + Move) on a custom Verkle-rooted state lane with cross-parity against `LTPAnchorRegistry`.

Phase 1 timeline: 17 weeks (8 sprints). Started 2026-04-23. Target close: Q1 2027.

## Load-bearing invariants

These are non-negotiable. Code that weakens them does not ship.

1. **Lane separation** — `gsxdb-lane` (data ingest) cannot directly mutate `gsxdb-state` (authoritative state). All mutations go through `gsxdb-bridge`. Enforced by `scripts/check-lane-separation.sh` and `deny.toml`. The `lane-auditor` subagent reviews every change to those paths.

2. **Proposition 1 (dual-VM consistency)** — at every checkpoint, `EVM balanceOf(addr) == Move Coin.value(addr)` for every address. Enforced by the property test in `gsxdb-state/tests/dual_vm_parity.rs` (10k+ iterations).

3. **Cross-parity** — Solidity `LTPAnchorRegistry` and Rust `gsxdb-anchor` must accept and reject the same inputs in the same way for all 36 entity-state-machine pairs. The `parity-checker` subagent reviews every change to anchor validation, FSM, or record layout.

4. **No git rebase, ever** — repo convention. Use `git merge` or `git pull --no-rebase`.

5. **No "Co-Authored-By" lines in commit messages** — repo convention.

## Workflow

You and the human collaborate sprint-by-sprint. The standard flow:

1. Human types `/sprint S<n>` to start a sprint
2. You read the spec, plan, get approval, implement step-by-step
3. You run `/check` for verification + invoke specialist subagents on sensitive surfaces
4. You prepare a PR with `gh pr create` (in `ask` permission tier — human confirms)
5. Human reviews and merges

For large sprints (S6 Verkle, S8 DAG), the human delegates to the `sprint-runner` subagent which drives end-to-end.

Between sessions, you resume via:

- This `CLAUDE.md` (sprint backlog section below)
- `gh issue list --label sprint-<n>` for tracked work
- Branch name (`<scope>/<short-slug>`) for context
- `.sprint-state.md` at the branch root for in-flight state

## Sprint backlog

| Sprint | Weeks  | Status      | Exit gate                                                           |
|--------|--------|-------------|---------------------------------------------------------------------|
| S1     | 1–2    | ✅ Closed    | Workspace + lane-separation script enforces (CI deferred)           |
| S2     | 3–4    | ✅ Closed    | PBM redb-backed BalanceStore + dual-projection invariant (3 layers) |
| S3     | 5–6    | ✅ Closed    | EVM + Move projector wiring; 10k-iter cross-VM parity invariant met |
| ~S3.5~ | —      | ❎ Dissolved | Per IQ-3, real-VM integration folds into S5; Move dialect is a launch-readiness call |
| S4     | 7–8    | ✅ Closed    | CE-MVCC OCC (Aptos Block-STM); parallel_equals_sequential @ 10k passing |
| S5     | 9–10   | ✅ Closed    | Cross-VM intent bundles + Intent::Call dispatch; bundle_atomicity @ 10k passing |
| S6     | 11–12  | ✅ Closed    | State-tree commitment (BLAKE3 per IQ-6); cross_tree_root_agreement @ 10k passing |
| S7     | 13–14  | ✅ Closed    | Cross-chain anchor log + parity (in-memory + MAC per IQ-7); cross_chain_parity_holds @ 10k passing |
| S8     | 15–16  | ✅ Closed    | Block store + recovery (in-memory per IQ-8); recover_matches_live_state @ 10k passing |
| S4     | 7–8    | ⏳ Queued    | CE-MVCC + OCC; 100k-iter serializability property test              |
| S5     | 9–10   | ⏳ Queued    | Cross-VM intent queue Q close; 10k-iter crash-recovery test         |
| S6     | 11–14  | ⏳ Queued    | Own-tree Verkle; N=10⁶ inclusion proof + go-ipa differential parity |
| S7     | 15–16  | ⏳ Queued    | Anchor log + 36-pair cross-parity green in CI                       |
| S8     | 17–18  | ⏳ Queued    | DAG store + recovery + telemetry; testnet shadow E2E                |

Update this table when a sprint closes.

## Conventions

### Build tools

- **Rust:** `cargo` for everything. Workspace at repo root.
- **Solidity:** `forge` (Foundry). Lives in `contracts/`.
- **Infra:** `terraform`. Lives in `terraform/`. Apply via `scripts/bootstrap.sh deploy-aws`, never raw `terraform apply` (that's denied).

### Branch naming

`<scope>/<short-slug>` — e.g., `state/pbm-rocksdb-cf`, `verkle/ipa-singlepoint`, `anchor/fsm-transition-table`.

For IQ-driven branches: `iq/<short-slug>`.

### Commits

- Focused, single-purpose
- Imperative mood ("Add X", not "Added X")
- No "Co-Authored-By"
- Reference issues with `Closes #N` or `Refs #N`

### Pull requests

- Title: matches the sprint or IQ context
- Body: must include exit gate test command + subagent verdicts
- Do not auto-merge; the human approves and merges

### Tests

- Unit tests inline (`#[cfg(test)] mod tests`)
- Integration tests in `tests/`
- Property tests use `proptest` — minimum 10k iterations for invariants
- Conformance fixtures in `tests/parity-fixtures/` (shared with Solidity)

### Migrations

The state schema is content-addressed and self-validating; no Alembic-style migrations. RocksDB CFs are added at startup if missing (`gsxdb-state::ensure_cfs()`).

## Specialist subagents

Invoke these proactively per the rules below.

| Trigger | Subagent | Why |
|---|---|---|
| Changes to `gsxdb-lane`, `gsxdb-bridge`, `scripts/check-lane-separation.sh`, `deny.toml` | `lane-auditor` | Lane-separation invariant |
| Changes to `gsxdb-verkle`, signature paths, KEM | `crypto-reviewer` | Cryptographic correctness + side-channels |
| Changes to anchor validation, FSM, `AnchorRecord` layout | `parity-checker` | Solidity ↔ Rust parity |
| Driving a full sprint end-to-end | `sprint-runner` | Multi-day autonomous run |

When a PR touches multiple surfaces, invoke specialists in parallel.

## Slash commands

| Command | What it does |
|---|---|
| `/sprint <id>` | Drive a sprint from start to PR |
| `/check` | Run all local verifications |
| `/release <version>` | Tag and ship a release |
| `/aws-status` | Snapshot AWS infra health (read-only) |
| `/audit-bridge` | Review lane-crossing changes |
| `/cross-parity` | Run 3-way `LTPAnchorRegistry` parity test |
| `/iq-decision <topic>` | Record a new IQ (Investigation Question) |

## Permissions

`claude-code/settings.json` defines three tiers:

- **Allowed silently** — read-only ops, local builds/tests, file ops, lane-separation script
- **Asked** — anything that mutates remote state (push, tag, PR creation, releases, AWS deploys)
- **Denied** — destructive ops (`rm -rf /`, force push, `terraform destroy`, `aws ec2 terminate`)

The denylist is the security floor. Add to it; do not remove without explicit security review.

## Hooks

`settings.json` configures:

1. **PostToolUse on Edit/Write/MultiEdit** — runs `cargo fmt --check`; hints if drift.
2. **PreToolUse on Bash** — pattern-blocks `rm -rf /`, `git push --force`, `terraform destroy` even if they sneak past the denylist.

## Resuming work

When a session opens cold:

1. Read this `CLAUDE.md` (already loaded).
2. Run `git status` and `git rev-parse --abbrev-ref HEAD` to see where the previous session left off.
3. If on a sprint branch, read `.sprint-state.md` at root.
4. Run `gh pr list --state open` to see in-flight PRs.

That's enough state to pick up cleanly without asking the human to re-orient.

## Updating this file

Update `CLAUDE.md` when:

- A sprint closes (mark it ✅ in the backlog table; bump the next one to 🟡)
- A new load-bearing invariant is added (rare; needs an IQ first)
- A new slash command or subagent is canonicalized
- A repo convention shifts

Treat changes to `CLAUDE.md` like any other PR — review and merge.
