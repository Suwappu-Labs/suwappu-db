---
description: Drive a sprint from start to PR — plan, implement, verify, prepare PR
argument-hint: <sprint-id e.g. S2, S3, ...>
allowed-tools: Read, Write, Edit, Glob, Grep, Bash, Agent
---

# Sprint $ARGUMENTS

You are driving sprint **$ARGUMENTS** of GSX-DB Phase 1.

## Phase 1: Orient

1. Read `CLAUDE.md` (top-level) — confirm current sprint backlog state.
2. Read the sprint spec for `$ARGUMENTS` (look in `docs/spec/`, `docs/sprints/`, or research notes — find it).
3. Run `gh issue list --label sprint-$ARGUMENTS` to see tracked issues.
4. Check current branch: `git rev-parse --abbrev-ref HEAD`. If not on a sprint branch, propose one (`<scope>/<short-slug>`) and create it after approval.
5. If `.sprint-state.md` exists on the branch, read it to resume. Otherwise create one.

Report findings before writing any code.

## Phase 2: Plan

Produce a written plan covering:

- **Exit gate** — the specific test(s) or property that must pass for the sprint to close
- **Files to touch** — list with one-line justification each
- **Tests to add** — name them, describe what they verify
- **Risks / load-bearing invariants** — anything touching lane separation, Proposition 1, cross-parity, or cryptographic correctness
- **Subagents to invoke** — see CLAUDE.md "When to invoke which subagent"

Wait for approval before implementing.

## Phase 3: Implement

Work step-by-step. After each meaningful step:

1. Run `cargo check` (or the relevant build) — must pass.
2. Run targeted `cargo test` — must pass.
3. Update `.sprint-state.md` with what's done and what's next.
4. Commit with a focused message (no Co-Authored-By).

If you hit a load-bearing invariant or ambiguity, **stop and ask**. Don't paper over.

## Phase 4: Verify

Before declaring sprint exit:

1. Run `/check` (the verification slash command) — must be all green.
2. Invoke specialist subagents per the rules in CLAUDE.md.
3. Confirm exit gate passes with the specific test invocation.
4. Update `CLAUDE.md` "Sprint backlog" section to reflect closure.

## Phase 5: PR

Run `gh pr create` (in `ask` permission tier — user will confirm).

PR description must include:

- Sprint exit gate and the test command that proves it
- Subagent reviews summarized (lane-auditor / crypto-reviewer / parity-checker output)
- Any deferred items moved to follow-up issues

Do **not** mark the PR ready-to-merge — that's the human's call.
