# GSX-DB Phase-1 Specification

This directory holds the formal spec, organised by sprint. Each sprint that
introduces new types, invariants, or storage layouts adds its own document
here. The `/sprint` slash command reads from this directory to ground its
implementation plans.

The spec evolves alongside the code. When an [IQ](../iq/) decision changes
spec semantics, the IQ's "Propagation checklist" must include updating the
relevant doc here.

## Spec layout

| File                     | Sprint | Status                  |
|--------------------------|--------|-------------------------|
| `lane-separation.md`     | S1     | implicit in code + script (TODO: lift to spec) |
| `pbm-balance-slot.md`    | S2     | implicit in code (TODO: lift to spec) |
| `dual-vm-projectors.md`  | S3     | not yet written         |
| `ce-mvcc-occ.md`         | S4     | not yet written         |
| `cross-vm-intent-queue.md` | S5   | not yet written         |
| `verkle-state-tree.md`   | S6     | not yet written         |
| `anchor-log.md`          | S7     | not yet written         |
| `dag-store-recovery.md`  | S8     | not yet written         |
| `storage.md`             | cross  | not yet written (also propagates IQ-1) |

## Convention

Each spec doc has the structure:

```
# <Title>

## Goal
One paragraph: what does this component do, and why is it part of the
phase-1 design?

## Types and invariants
The Rust types it introduces, and the invariants those types enforce
(structurally where possible, by property test where not).

## Storage layout
For storage-touching components: what tables / column families / on-disk
representations exist. Encodings.

## Failure model
What can go wrong, what gets reported, what gets retried, what gets logged.

## Tests
The exit-gate test that closes the sprint, and the property tests that
guard the invariants under change.

## Open questions
IQ candidates surfaced during implementation that didn't block the sprint.
```

Drafts are fine — keep specs accurate to the shipped code, not to the
original sketch.
