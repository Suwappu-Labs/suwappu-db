# Line-by-line audit plan for placeholder facts

This document is the execution plan to audit the repo line-by-line, find
fact placeholders, and convert each into an explicit implementation task.

## Goal

- Identify every placeholder/deferred statement across code + docs.
- Classify each item as one of: `implemented`, `intentional defer`,
  `stale/incorrect`, or `missing spec`.
- Create a prioritized work queue so implementation can proceed without
  ambiguity.

## Audit workflow (strict)

1. **Enumerate candidate lines** using fixed search patterns (no manual sampling).
2. **Review every match line-by-line** in file context (±20 lines).
3. **Assign classification** and a required action.
4. **Create/refresh IQ or sprint task** for every non-implemented line.
5. **Record disposition** in the audit ledger table.
6. **Open execution PRs** grouped by subsystem (state, VM, anchor, recovery).

## Search patterns to run

```bash
rg -n "TODO|TBD|placeholder|deferred|out of scope|mock|launch-readiness|Phase-1 ships" crates docs CLAUDE.md
rg -n "TODO|FIXME|XXX" crates docs scripts
rg -n "not yet|future work|to be added|contingency" docs crates
```

## Scope buckets (audit in this order)

1. **Runtime correctness surfaces (code first)**
   - `crates/suwappudb-bridge/src/recovery/*`
   - `crates/suwappudb-bridge/src/anchor/*`
   - `crates/suwappudb-bridge/src/vm/*`
   - `crates/suwappudb-state/src/tree/*`
2. **Invariant and property tests**
   - `crates/suwappudb-bridge/tests/*`
   - `crates/suwappudb-state/tests/*`
3. **Normative specs/docs**
   - `docs/spec/*`
   - `docs/architecture/*`
   - `docs/iq/*`
4. **Project control docs**
   - `CLAUDE.md`

## Ledger format (required for every matched line)

| ID | File:Line | Placeholder text | Classification | Required action | Owner sprint |
|---|---|---|---|---|---|
| A-001 | `path:line` | short quote | defer / stale / implemented / missing-spec | concrete code/doc change | S8.5+ |

Rules:
- One row per matched line (no grouping-by-file).
- If two lines imply different actions, use two rows.
- Every `stale` row must have a fix PR before closing the audit.

## Initial hotspot list (starting points)

These are already known high-impact placeholders to verify first:

- `crates/suwappudb-lane/src/lib.rs:32` (phase-1 mempool placeholder)
- `crates/suwappudb-bridge/src/recovery/replay.rs:77` (genesis-parent placeholder comment)
- `crates/suwappudb-state/src/tree/ops.rs:127` (placeholder commitment path)
- `crates/suwappudb-state/src/tree/commit.rs:37` (const placeholder commitment)
- `docs/spec/README.md:16-17` (spec TODO markers)
- `docs/architecture/overview.md:110` (`TBD` runtime row)
- `docs/architecture/dual-projection.md:118-122` (explicit placeholder IQ refs)

## Delivery plan (audit → implementation)

### Step 1 — Build the ledger
- Run search patterns.
- Review each match in context.
- Populate `docs/architecture/audit-ledger.md` with one-row-per-line.

### Step 2 — Freeze priorities
- Tag each row by severity:
  - `P0` correctness/security risk
  - `P1` behavior/spec mismatch
  - `P2` docs hygiene
- Convert P0/P1 rows into sprint tasks.

### Step 3 — Execute code fixes first
- Land P0/P1 runtime fixes in small PRs.
- Keep each PR scoped to one subsystem.
- Add/adjust tests with each fix.

### Step 4 — Reconcile specs/docs
- Update architecture/spec/IQ docs only after code behavior is final.
- Remove stale placeholder language.
- Keep intentional defers explicit with owning sprint.

### Step 5 — Closeout gate
Audit is complete only when:
- No uncategorized placeholder lines remain.
- All `stale` rows are fixed.
- All intentional defers point to a concrete sprint/IQ owner.
- Ledger is linked from `docs/architecture/README.md`.

## Definition of done

- A reproducible, line-level ledger exists.
- Runtime placeholders have implementation tickets/PRs.
- Docs match code and sprint reality.
- The next sprint backlog is backed by audited facts, not assumptions.
