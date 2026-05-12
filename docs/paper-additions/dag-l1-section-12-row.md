# Insertion: DAG L1 paper, additions to Table 1 (§12)

**Where:** Table 1 (Classical-cryptography exception zones at
launch), §12 — append two rows so the substrate-level placeholders
are disclosed identically to the protocol-level ones.

---

## Updated Table 1

| Surface | Algorithm | Migration target |
|---|---|---|
| EVM account TX signing | ECDSA secp256k1, hybrid-composed with ML-DSA-65 | Pure ML-DSA-65 by ∼2030 |
| LTP aggregate signatures | BLS12-381 | Hash-based + SP1-STARK aggregation, 2027–2029 |
| Optional verification mode | Groth16 over BN254 | Default to FRI by ∼2030; Groth16 retained as opt-in |
| **State-tree commitment (§7.4.5)** | **BLAKE3 placeholder over a 256-ary trie** | **IPA over banderwagon (Verkle) at 2026 Q4 / Mainnet Beta** |
| **Phase-1 anchor authentication (§7.4.7)** | **BLAKE3 keyed-MAC** | **ECDSA secp256k1 + ML-DSA-65 hybrid on the corridor super-node surface, aligned with §12 row 1** |

The two new rows are substrate-level analogues of the existing
protocol-level exception zones. Their migration paths are folded
into the unified post-quantum migration roadmap.

### Why disclose

The papers' own §12 establishes the discipline that
classical-cryptography surfaces be enumerated rather than
suppressed. We extend the same discipline to the substrate: the
phase-1 commitment and authentication primitives are placeholders
with documented swap points (`commit_node` and `Anchor::compute_mac`
respectively), not durable production primitives.

### Witness-size disclosure (carry-over)

Witness sizes for the phase-1 BLAKE3 commitment versus
launch-readiness IPA:

| Proof type | Phase-1 (BLAKE3) | Launch (IPA over banderwagon) |
|---|---|---|
| Inclusion (worst case) | ≈ 163 KB | ≈ 200 B |
| Absence (early termination) | depth × 255 × 32 B (depth ≤ 20) | ≈ 200 B |
| Empty tree | 0 B | 0 B |

Stateless-light-client compatibility is gated on the swap to IPA.
Phase-1 stateful nodes are unaffected.
