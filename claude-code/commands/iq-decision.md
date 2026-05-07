---
description: Record a new IQ (Investigation Question) — a numbered architectural decision with rationale
argument-hint: <topic — short slug>
allowed-tools: Read, Write, Edit, Glob, Grep, Bash
---

# Record IQ: $ARGUMENTS

You are recording a new IQ (Investigation Question) for GSX-DB. IQs are numbered architectural decisions that propagate to the spec, the research doc, the relevant ADR, and `CLAUDE.md`.

## Phase 1: Determine the IQ number

```bash
# Find the highest existing IQ number across the repo
grep -roh "IQ-[0-9]\+" docs/ CLAUDE.md 2>/dev/null | sort -u | sort -V | tail -5
```

Pick the next integer. Confirm with the user before proceeding.

## Phase 2: Draft the IQ

Format:

```markdown
## IQ-N: <Title — restating $ARGUMENTS>

**Status:** Proposed
**Date:** <today YYYY-MM-DD>
**Sprint context:** <which sprint surfaced this, or "cross-cutting">

### Question
<one paragraph — what's the open question or tradeoff?>

### Context
<two to four paragraphs — what observations led to this question? what constraints
are at play? what does the current design assume that's now in tension?>

### Options considered
1. **<option A>** — <one line>
   - Pros: <list>
   - Cons: <list>
2. **<option B>** — <one line>
   - Pros: <list>
   - Cons: <list>

### Decision
<chosen option, with explicit rationale>

### Consequences
- **Spec changes:** <which sections of docs/spec/ need updating>
- **ADR changes:** <which ADRs need updating or superseding>
- **Code changes:** <which crates / modules are affected>
- **Test changes:** <new property tests, conformance fixtures, etc.>

### Propagation checklist
- [ ] Update `docs/spec/<relevant>.md`
- [ ] Update `docs/research/<relevant>.md`
- [ ] Update or supersede `docs/adr/<NNNN>-<slug>.md`
- [ ] Update `CLAUDE.md` if it changes load-bearing invariants
- [ ] Update affected code per the consequences above
```

## Phase 3: Show draft to user

Print the full draft. Wait for approval or revisions.

## Phase 4: Persist

Once approved:

1. Branch: `git switch -c iq/<short-slug>` (e.g., `iq/q-max-depth`)
2. Write the IQ to `docs/iq/IQ-N-<slug>.md`
3. Apply the propagation checklist — edit each file in the list
4. Commit with message: `iq: IQ-N <title>`

Do **not** create the PR automatically — let the user do `gh pr create` or invoke `/sprint`-style PR creation when ready.
