## IQ-6: State-tree commitment scheme — hash-based in phase-1, real Verkle at launch readiness

**Status:** Accepted
**Date:** 2026-05-08
**Sprint context:** S6 (state tree)

### Question

S6 calls for a Verkle state tree. Real Verkle uses polynomial
commitments (IPA over the banderwagon curve) at every internal node,
yielding ~200-byte witnesses regardless of how many keys are proven.
This is the "stateless light client" enabler.

But the heavy-crypto dep tree mirrors the Move-VM problem from IQ-3
and the RocksDB problem from IQ-1: pulling production cryptography
into a phase-1 codebase is high cost (build time, alignment with the
broader ecosystem) for properties that aren't yet load-bearing.

### Context

S6's correctness claims are:

- Deterministic root commitment per block (same state ⇒ same root)
- Root changes when state changes (no false negatives)
- Inclusion proofs verify; non-inclusion is detectable

These properties are independent of which commitment scheme is used.
A hash-based commitment satisfies them. The thing real Verkle adds is
**witness compression** — proofs go from O(depth × 256 × 32 bytes)
worst-case to ~200 bytes regardless. That matters for stateless clients
and full-node bandwidth, but is not a phase-1 correctness gate.

### Options considered

1. **Real Verkle (IPA over banderwagon).**
   - Pros: stateless-client compatible, small witnesses, the canonical
     Ethereum direction.
   - Cons:
     - Pulls `ark-bls12-381` or equivalent + `crate-crypto/banderwagon` +
       polynomial-commitment + IPA prover/verifier — substantial dep tree.
     - Build time: meaningful (curve operations, multi-scalar
       multiplication, FFT).
     - Trusted-setup-free (IPA), so no ceremony issue, but the curve
       itself is bleeding-edge.
     - The Ethereum Verkle spec is still settling (post-Pectra). Picking
       a specific Rust impl now risks needing to migrate when the spec
       finalises.

2. **Hash-based 256-ary trie (BLAKE3).**
   - Pros:
     - 5 transitive deps, builds in seconds, no curve crypto.
     - Same tree shape as Verkle (256-ary, 20-deep over 20-byte
       addresses) so swap is mechanical.
     - Witness format is similar (commitments at each level + sibling
       map per step); only the inner commitment function changes.
   - Cons:
     - Witnesses are ~5KB worst case (20 levels × up to 255 sibling
       commitments × 32 bytes). Fine for full nodes; not for stateless
       light clients.

3. **Defer S6 entirely.**
   - Pros: keeps spec intact, real Verkle when launch needs it.
   - Cons: leaves a hole in the architecture — no state-root commitment
     means downstream sprints (S7 anchor log) have nothing to anchor.
     S7 *needs* a per-block state root.

### Decision

**Option 2** — hash-based 256-ary trie with BLAKE3 commitments in
phase-1 S6. Real Verkle becomes a launch-readiness item parallel to
IQ-3's Move VM choice.

The tree shape, depth, traversal logic, and proof format are all
Verkle-aligned. The single function that changes when real Verkle
lands is `commit_node`, which becomes an IPA polynomial commitment
instead of a BLAKE3 hash. The trait surface, the proof structure, and
every test stay identical.

### Why this isn't kicking the can

- **S7 (anchor log) gets a real state root to anchor.** Cross-chain
  parity verification works against the hash-based root. When real
  Verkle replaces it, the same anchors point to the new root format.
- **The properties that matter for downstream sprints are verified.**
  Determinism, change-detection, inclusion/absence proofs — all 10k-case
  property-tested. Witness *size* is the only deferred property.
- **The swap point is well-defined.** `commit::commit_node` and a
  matching `verify` function. ~50 LOC change, no architecture impact.
- **Real Verkle is bleeding-edge.** Locking in any Rust impl today
  means tracking that impl through Ethereum spec changes. Waiting
  until launch readiness lets the upstream stabilise.

### Consequences

- **Spec changes:** `docs/spec/verkle-state-tree.md` (this slice's
  spec doc) clearly notes the BLAKE3-now / Verkle-later split.
- **Code changes:**
  - `crates/gsxdb-state/src/tree/`: 256-ary trie + BLAKE3 commitments.
  - `BalanceStore::entries()`: required for `StateTree::from_state`.
  - `BlockReport`: gained `state_root: Commitment`.
- **Test changes:** All proptest properties run against BLAKE3. Same
  tests run against real Verkle when it lands.
- **Witness size disclosure:** Public-facing docs must note that
  phase-1 witnesses are large until real Verkle ships. Stateless
  client work is gated on it.

### What this leaves open

- **Launch-readiness Verkle integration.** Same parent decision as
  IQ-3's Move VM: when the chain prepares for testnet/mainnet, choose
  a Rust Verkle impl (rust-verkle, ipa-multipoint, hand-rolled), wire
  it in, re-run the property tests. Likely IQ-7 in time.
- **Incremental tree updates (S6.5 / S8).** Phase-1 rebuilds the
  entire tree per block. Production needs incremental, but the trait
  surface is unchanged.
- **Persistent tree storage.** Same — tied to S8 recovery.

### Propagation checklist

- [x] Code: `gsxdb-state::tree` ships with BLAKE3 commitments
- [x] Tests: 24 unit + 6 proptests, exit gate at 10k cases
- [x] Block-level integration: `BlockReport::state_root`
- [x] `docs/spec/verkle-state-tree.md` written and references this IQ
- [ ] Launch-readiness checklist: add "wire real Verkle" alongside
      Move VM choice (IQ-3) — checklist itself doesn't yet exist
- [ ] Public docs (when written) call out the witness-size caveat
