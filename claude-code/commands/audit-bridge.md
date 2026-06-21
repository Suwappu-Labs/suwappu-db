---
description: Review changes that cross or affect the lane-separation boundary
allowed-tools: Read, Glob, Grep, Bash, Agent
---

# Audit lane-bridge changes

The lane-separation invariant — that data lanes (`suwappudb-lane`) cannot directly mutate state lanes without going through the bridge (`suwappudb-bridge`) — is **load-bearing**. Any PR that touches the bridge crates, the lane-separation script, or `deny.toml` linting rules deserves explicit review.

## Phase 1: Gather changes

Determine what changed:

```bash
# Compare current branch against main
git diff main...HEAD --stat
git diff main...HEAD -- suwappudb-lane/ suwappudb-bridge/ scripts/check-lane-separation.sh deny.toml
```

If the diff is empty in those paths, report "no lane-bridge surface touched" and stop.

## Phase 2: Delegate to lane-auditor

Invoke the `lane-auditor` subagent with:

- The diff above as context
- The full file contents of any modified bridge file
- A request: "Verify that this change does not weaken the lane-separation invariant. Flag any new direct read or write paths from data lane → state lane that bypass the bridge. Report: SAFE / SUSPICIOUS / UNSAFE with specific line references."

## Phase 3: Cross-check the script

If `scripts/check-lane-separation.sh` changed, manually walk its logic. The script must:

1. Enumerate all modules in `suwappudb-lane/`
2. For each, statically check that no item imports from `suwappudb-state` directly (only `suwappudb-bridge`)
3. Exit non-zero on any violation

If those properties have weakened, **flag it as UNSAFE regardless of what the auditor says**.

## Phase 4: Report

```
File                         Risk      Notes
suwappudb-bridge/src/lib.rs      Low       Added new approve_anchor() — bridge-mediated, OK
suwappudb-lane/src/queue.rs      Medium    Imports from suwappudb-state — verify it goes through bridge
deny.toml                    High      Rule disabling the lane-cross check was removed
```

End with one of: `SAFE TO MERGE`, `NEEDS CHANGES`, or `BLOCK — invariant weakened`.
