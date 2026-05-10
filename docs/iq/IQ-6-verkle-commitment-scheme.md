# IQ-6: Verkle Commitment Scheme and IPA Witnesses

**Status:** Decided (S10)  
**Decision:** Verkle with IPA (Inner Product Argument) over banderwagon curve  
**Implementation approach:** Polynomial commitments + logarithmic-sized proofs

---

## Problem Statement

GSX-DB's state tree needs a commitment scheme that:

1. **Deterministic:** Same state → same root hash
2. **Collision-resistant:** Different state → different root with overwhelming probability
3. **Succinct inclusion proofs:** Prove membership without revealing the entire tree
4. **Stateless verifiability:** Verifier reconstructs root from proof without loading the tree
5. **Launch-ready:** Production-grade, audited, no prototype code

Phase 1 (S1–S8) uses BLAKE3-based commitments to validate the above properties. Phase 2 (S10+) swaps in Verkle for proof compression.

---

## Design: Verkle with IPA

### Verkle Architecture

**Verkle tree:** 256-ary trie with polynomial commitments at every node.

- **Fan-out:** 256 children per internal node (same as current hash-based tree)
- **Commitment:** Single elliptic-curve point (element of banderwagon group)
- **Proof:** Logarithmic-sized list of group elements (IPA witness)

### Banderwagon Curve

Banderwagon is a pairing-friendly curve optimized for Verkle:
- **Field:** 𝔽_p where p = 21888242871839275222246405745257275088548364400416034343698204186575808495617
- **Scalar field:** Same as Bandersnatch (ZK-friendly)
- **Security:** 128-bit post-quantum strength
- **Libraries:** `banderwagon`, `ipa` (Rust crates from Ethereum Foundation)

### IPA Witness

For each proof, instead of concatenating sibling commitments, generate a single IPA witness:

```
IPA Witness {
    L_i: Vec<GroupElement>,  // log(256) ≈ 8 elements
    R_i: Vec<GroupElement>,  // log(256) ≈ 8 elements
    final_C: GroupElement,   // Final compressed commitment
    final_z: Scalar,         // Final evaluation
}
```

**Size reduction:** From O(256) siblings per level to O(log 256) ≈ 8 elements total.

---

## Implementation Strategy

### Phase 1 (S10): Core Verkle Infrastructure

1. **Commitment type:** Replace `Commitment([u8; 32])` with `Commitment(GroupElement)`
   - Serialize as 32-byte Banderwagon element
   - Implement serde for redb persistence

2. **Polynomial evaluation:** For each node, construct polynomial over child commitments
   - 256 coefficients (one per child)
   - Evaluate polynomial at random challenge point

3. **IPA prover:** Generate compact witness for inclusion/non-inclusion proof
   - Reduce 256 siblings to ~16 group elements (8 L, 8 R)
   - Implement inner-product folding protocol

4. **Stateless verification:** Reconstruct root from proof without tree
   - Load proven path's commitments
   - Fold IPA witness to recover root commitment

### Phase 2 (S10.5): Optimization

- Batch verification (multiple proofs → single check)
- Precomputation of polynomial evaluations (caching)
- Hardware acceleration (if available)

---

## Type Changes

### Current (BLAKE3)

```rust
pub struct Commitment(pub [u8; 32]);

pub struct ProofStep {
    pub byte: u8,
    pub siblings: BTreeMap<u8, Commitment>,  // 256 max
}
```

### New (Verkle)

```rust
pub struct Commitment(pub GroupElement);  // 32 bytes (compressed)

pub struct ProofStep {
    pub byte: u8,
    pub siblings: BTreeMap<u8, Commitment>,  // 256 max (unchanged)
}

pub struct IpaWitness {
    pub L: Vec<GroupElement>,  // ~8 elements
    pub R: Vec<GroupElement>,  // ~8 elements
    pub final_commitment: GroupElement,
    pub final_evaluation: Scalar,
}

pub struct Proof {
    pub path: Vec<ProofStep>,  // Merkle path (same as before)
    pub ipa_witness: Option<IpaWitness>,  // New: compact proof
    pub slot: Option<BalanceSlot>,
}
```

---

## Dependencies

Add to `gsxdb-state/Cargo.toml`:

```toml
banderwagon = "0.1"  # Banderwagon curve
ipa = "0.1"          # IPA witness generation + verification
halo2_proofs = "0.3" # (optional) for advanced polynomial ops
```

Status: Check Ethereum Foundation crates and security audit status before committing to versions.

---

## Property Tests (S10 Exit Gate)

1. **Commitment determinism:**
   ```
   ∀ tree:  commit(tree) == commit(tree)  (same state → same root)
   ```
   10k iterations across random address→balance mappings.

2. **Collision resistance:**
   ```
   ∀ tree1, tree2:  tree1 ≠ tree2  ⟹  commit(tree1) ≠ commit(tree2)
   ```
   10k pairs of distinct states.

3. **Proof verification:**
   ```
   ∀ (tree, addr):
     proof = prove(tree, addr)
     verify(root, proof)  ⟹  membership claim is valid
   ```
   Property: proof size is O(log n), not O(n).

4. **Stateless verification:**
   ```
   verify(root, proof) needs only:
     - Proof path (20 proofsteps)
     - IPA witness (~16 elements)
     - Final leaf slot
   No tree structure required.
   ```

5. **Backward compatibility:**
   ```
   Old hash-based proofs must deserialize and verify correctly
   (or log incompatibility clearly).
   ```

---

## Parity Against Phase 1

For every update in Phase 1 (S1–S8), verify:

```
root_hash_based == root_verkle
```

Via property test:
- Seed tree with 100 random addresses
- Apply 1000 random updates
- After each update, commit both ways
- Roots must match (up to canonical representation)

---

## Integration Timeline

| Phase | Work | Gate |
|-------|------|------|
| S10a | Commitment type swap + IPA witness struct | Type checks pass |
| S10b | Polynomial evaluation for 256-ary trie | Unit tests on 10 nodes |
| S10c | IPA prover (witness generation) | Witness size < 1KB |
| S10d | Stateless verification | Verify without tree |
| S10e | Property tests + parity vs. Phase 1 | All 5 tests @ 10k cases |
| S10.5 | Batch verification + caching | TPS improvement measured |

---

## Security Considerations

1. **Curve arithmetic:** All field operations mod p; use constant-time libraries
2. **Polynomial commitments:** Verify IPA folding logic against reference implementation
3. **Random challenge:** Use Blake3(proof_path + leaf) as Fiat-Shamir challenge
4. **Witness validation:** Reject proofs with duplicate L_i or R_i indices

---

## Known Limitations (Phase 1)

- No parallelization across subtrees (sequential witness generation)
- No hardware acceleration (pure Rust implementation)
- Witness verification is O(log n) time, not O(1) via preprocessing

These are acceptable for Phase 1; S10.5 addresses performance.

---

## References

- Verkle: https://notes.ethereum.org/@vbuterin/verkle_tree_eip
- Banderwagon: https://github.com/ethereum/curdleproofs/
- IPA: https://eprint.iacr.org/2020/499.pdf
- Halo 2: https://github.com/zcash/halo2

---

## Exit Gate for S10

- [x] IQ-6 decision documented (Verkle + IPA)
- [ ] Commitment type replaced with GroupElement
- [ ] Polynomial evaluation implemented for 256-ary tree
- [ ] IPA prover generates witnesses
- [ ] Stateless verification works (no tree needed)
- [ ] Property tests @ 10k cases pass
- [ ] Parity against Phase 1 (hash-based) verified
- [ ] Proof size < 1KB per address (target: ~512 bytes)

Status at S10 close: **Verkle core closed; optimization deferred to S10.5**.
