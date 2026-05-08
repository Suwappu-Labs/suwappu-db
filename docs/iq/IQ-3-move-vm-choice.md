## IQ-3: Move VM choice — defer to chain-launch decision, hand-rolled interpreter as contingency

**Status:** Accepted
**Date:** 2026-05-08
**Sprint context:** S3.5 (real-VM swap-in), follow-up to IQ-2

### Question

S3.5 calls for a real Move VM to replace `MockMove`. Which Move VM
implementation does this project use:

1. Aptos `move-vm-runtime` (from `aptos-core`)
2. Sui's fork (from `MystenLabs/sui`)
3. Upstream `move-language/move`
4. Hand-rolled minimal Move interpreter
5. Defer the choice; keep mocks until forced

This decision affects the chain's Move dialect, framework lock-in,
upgrade story, and positioning relative to other Move chains.

### Context

Move has fragmented since the Diem era. The three production-grade
forks (Aptos, Sui, Movement) have diverged at the language level —
abilities syntax, object model, native function set, framework
bindings, gas semantics. Picking one is implicitly aligning the chain's
Move surface to that fork's idioms.

The reverse is also true: not picking is a position. Phase-1 has
shipped the structural form of the dual-projection invariant
(`BalanceSlot` with EVM and Move projections that are equal by
construction) and S3 has shipped the operational form at the canonical
layer (mock executors, 10k-case proptest). What's deferred is
verification at real Move bytecode level — and that verification is
only meaningful once the dialect is chosen, because the mock and the
real Move do the same thing for transfers regardless.

No phase-1 sprint forces the choice:

- **S4 (CE-MVCC OCC)** — scheduler work over the canonical state. VM-agnostic.
- **S5 (cross-VM intent queue)** — modelling contract calls. Can be done at the canonical layer initially; real-VM verification is a follow-up.
- **S6 (Verkle state tree)** — state commitment. VM-agnostic.
- **S7 (anchor log)** — cross-chain settlement. VM-agnostic.
- **S8 (DAG store + recovery)** — durability. VM-agnostic.

The first sprint that *intrinsically* needs real Move semantics is
whatever exposes Move modules to user code (a dev-net or testnet
launch milestone, not a phase-1 sprint).

### Options considered

#### Option 1 — Aptos `move-vm-runtime`

- Production-tested at scale (Aptos mainnet)
- Pulls the entire Aptos framework as a transitive dep tree
- Aptos-flavoured Move (`aptos_framework::coin::Coin<T>` semantics, ability set, gas)
- Strategic alignment: chain becomes "an Aptos-Move chain"
- Build cost: hundreds of crates, multi-GB target dir
- Maintenance: handed; we'd track Aptos releases

#### Option 2 — Sui's fork (vendored)

- Modern, actively developed
- Object-centric model, Sui-flavoured Move
- Not packaged for external use; vendoring is mandatory
- Strategic alignment: chain becomes "a Sui-Move chain"
- Vendoring is a real ongoing maintenance cost — every Sui upgrade is a merge problem
- Build cost: even larger than Aptos
- Maintenance: ours, including security backports

#### Option 3 — Upstream `move-language/move`

- The Diem-era canonical Move
- Smallest of the three real forks
- Less framework lock-in
- **Maintenance status is genuinely unclear** — Diem is dead; Aptos and Sui are the active forks; the upstream repo's commit cadence is sparse. Production use here means accepting we're a major user of a quiet codebase.
- Strategic alignment: closest to "vanilla Move," but no production chain actually runs upstream

#### Option 4 — Hand-rolled minimal Move interpreter

- Custom 500–1000-LOC Rust crate
- Models the subset of Move our chain actually uses (`Coin<T>`, `move_to`, `move_from`, `borrow_global`, simple entry functions)
- We invent a Move-shaped bytecode and an encoder
- Symmetric in spirit with `MockMove` but with real bytecode semantics
- **The honest framing: this is not a Move VM. It's a Move-shaped interpreter.** Production Move tooling (compilers, debuggers, source-level tooling) doesn't apply.
- Useful as a *contingency* if a sprint forces the question before the dialect is chosen
- Useful for property tests that need bytecode-level semantics without committing to a fork

#### Option 5 — Defer the choice

- Keep `MockMove` for all phase-1 sprints (S4–S8)
- Mark the Move VM choice as a launch-readiness milestone, not a sprint deliverable
- Phase-1 ships the structural and canonical forms of the invariant; bytecode-level verification waits
- Lets the Move ecosystem stabilise (Aptos vs Sui vs Movement vs newcomer) before we commit
- Avoids burning engineering effort on integration that may need to be redone

### Decision

**Option 5 (defer) as the primary path. Option 4 (hand-rolled) reserved
as a contingency.**

Concretely:

- **Phase-1 (S4–S8) ships with `MockMove`.** No real Move VM is wired
  in until the chain is preparing for testnet/mainnet, at which point
  the dialect choice becomes a launch-readiness decision with
  product/strategy weight, not a sprint deliverable.

- **If a phase-1 sprint surfaces a need for real Move-bytecode-level
  semantics before the dialect is chosen, the contingency is the
  hand-rolled minimal interpreter (Option 4).** It models the subset
  of Move semantics our property tests actually exercise. It is
  explicitly not a production Move VM and is replaced when the dialect
  is chosen.

- **S3.5 is rescoped.** With Move VM deferred, "S3.5" reduces to "real
  revm integration." Per IQ-2 the EVM half is straightforward but
  catches no new bugs until contract calls matter (S5). Recommend
  rolling real-revm into S5 as well, rather than building it now in
  isolation.

  **Net effect: S3.5 is dissolved into S5.** When S5 needs real EVM
  contract calls, real revm lands. When S5 needs real Move contract
  calls, the dialect decision is made.

### Why not pick a real Move VM now

Three reasons, in order of weight:

1. **Strategic, not technical.** Aptos/Sui/Movement are positioning
   bets. Making this call inside a coding sprint is the wrong forum.
2. **No sprint forces it.** Every phase-1 sprint can be completed with
   `MockMove`. Picking before forced is premature.
3. **The Move ecosystem is mid-fragmentation.** Picking now risks
   picking the loser.

### Consequences

- **Spec changes:** `docs/spec/dual-vm-projectors.md` "Open questions"
  updated to reference this IQ. "Failure model" notes the bytecode-
  level verification gap is intentionally deferred.
- **ADR changes:** None.
- **Code changes:** None directly. `MockMove` continues as the
  Move-side executor through phase-1.
- **Sprint plan changes:** S3.5 dissolved. Real revm and real Move VM
  fold into S5 (cross-VM intent queue) as scope, with the Move side
  gated on a separate launch-readiness call.

### Propagation checklist

- [x] `.sprint-state.md`: note S3.5 dissolution
- [x] `CLAUDE.md`: remove S3.5 row, note Move VM as launch-readiness item
- [x] `CHANGELOG.md`: IQ-3 entry under [Unreleased]
- [x] `docs/spec/dual-vm-projectors.md`: update "Open questions"
- [x] IQ-2 cross-references updated: S3.5 → "S5, plus a launch-readiness Move VM call"
- [ ] Launch-readiness checklist (does not exist yet): add "decide Move VM dialect" as a gate

### What this leaves open

- **IQ-4 (still open):** address-shape mismatch (EVM 20-byte vs Aptos
  Move 32-byte). Becomes urgent when the Move dialect is chosen.
- **IQ-5 (still open):** nonce semantics. EVM uses nonces; Move's
  signing model differs. Becomes urgent when real Move execution lands.
- **A new IQ:** when (which sprint or milestone) does the launch-
  readiness Move VM choice get made? Probably not in phase-1 at all.
