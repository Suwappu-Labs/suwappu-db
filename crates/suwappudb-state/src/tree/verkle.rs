//! Verkle tree with IPA (Inner Product Argument) polynomial commitments.
//!
//! Verkle replaces Phase 1's hash-based commitments with elliptic-curve
//! polynomial commitments, achieving O(log n) proof size instead of O(n).
//!
//! This module is available behind the `production-verkle` feature gate.
//! Phase 1 (S1–S8) uses the hash-based implementation by default.
//!
//! # Implementation
//!
//! - Each node commits to its children via a polynomial `C(x) = Σ[child_i * L_i(x)]`
//! - The commitment is a point on the banderwagon curve
//! - Proofs use IPA folding to compress witnesses from O(256) to O(log 256) ≈ 8 elements
//! - Verification is stateless: reconstructs root from proof without loading the tree

use std::fmt;

/// A banderwagon group element (curve point) representing a Verkle commitment.
///
/// Serializes as 32 bytes (compressed point representation).
///
/// In Phase 1, this is a placeholder. In S10, this wraps actual elliptic-curve arithmetic.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct GroupElement(pub [u8; 32]);

impl fmt::Debug for GroupElement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GroupElement(0x")?;
        for b in &self.0[..4] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "...)")?;
        Ok(())
    }
}

impl GroupElement {
    /// Create a group element from a 32-byte array.
    #[must_use]
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        GroupElement(bytes)
    }

    /// Serialize to 32 bytes.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

/// One IPA opening — proves that a single Lagrange-basis polynomial
/// (committed via banderwagon-IPA) evaluates to `claimed_value` at the
/// challenge point `query_point`.
///
/// The opening is the serialized `IPAProof` from `ipa-multipoint`:
/// `log2(domain_size) * 2 + 1` field elements = 17 × 32 B = 544 B for
/// our 256-wide domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaOpening {
    /// Commitment to the polynomial being opened (32 bytes).
    pub commitment: GroupElement,
    /// Domain index where the relation is being asserted (0..256).
    /// The verifier expands this into the Lagrange-coefficient vector.
    pub domain_index: u8,
    /// Claimed evaluation `f(domain_index)` as a 32-byte scalar.
    pub claimed_value: [u8; 32],
    /// Serialized `IPAProof` (`L_vec` + `R_vec` + a). Variable-length but
    /// deterministic for a fixed domain: 17 × 32 = 544 B for width-256.
    pub ipa_proof_bytes: Vec<u8>,
}

impl IpaOpening {
    /// Size in bytes of the opening (commitment + index + value +
    /// IPA proof). Used by the witness-size budget test in S10.6.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        32 /* commitment */ + 1 /* domain_index */ + 32 /* claimed_value */ + self.ipa_proof_bytes.len()
    }
}

/// IPA witness — one [`IpaOpening`] per internal node on the proof
/// path. Length matches `Proof.path.len()` minus the optional leaf
/// node (the leaf's polynomial opens at indices 0/1 to expose the
/// canonical balance; that opening is the last entry).
///
/// Provides O(log W) proof-size in W (the Lagrange domain width) per
/// path step, instead of the BLAKE3 path's O(W) sibling list per step.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct IpaWitness {
    /// One opening per internal node, in root-to-leaf order. The
    /// last entry corresponds to the leaf's polynomial commitment
    /// being opened at index 0 (low limb of the canonical balance).
    pub openings: Vec<IpaOpening>,
}

impl IpaWitness {
    /// Empty witness (used for non-inclusion proofs in an empty tree).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Total size in bytes — sum of each opening's `size_bytes`.
    pub fn size_bytes(&self) -> usize {
        self.openings.iter().map(IpaOpening::size_bytes).sum()
    }
}

/// Represents whether to use hash-based (Phase 1) or Verkle (S10+) commitments.
///
/// Distinct from [`super::commit::CommitmentScheme`] (the *trait* that
/// the BLAKE3 and banderwagon schemes implement) — this enum is a
/// runtime tag for telemetry / configuration. The trait dispatches
/// the actual commit; this enum names which trait impl is wired in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(Default)]
pub enum CommitmentSchemeKind {
    /// Hash-based (BLAKE3) for Phase 1.
    #[default]
    HashBased,
    /// Verkle with IPA for S10+.
    #[cfg(feature = "production-verkle")]
    Verkle,
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_element_from_bytes() {
        let bytes = [0x42u8; 32];
        let ge = GroupElement::from_bytes(bytes);
        assert_eq!(ge.to_bytes(), bytes);
    }

    #[test]
    fn ipa_opening_size_is_proof_plus_overhead() {
        // For domain-256, ipa-multipoint's IPAProof serializes as
        // log2(256) = 8 L points + 8 R points + 1 final scalar = 17 ×
        // 32 = 544 bytes. The full opening adds 32 (commitment) + 1
        // (index) + 32 (claimed value) = 609 bytes.
        let opening = IpaOpening {
            commitment: GroupElement([0; 32]),
            domain_index: 7,
            claimed_value: [0; 32],
            ipa_proof_bytes: vec![0u8; 17 * 32],
        };
        assert_eq!(opening.size_bytes(), 32 + 1 + 32 + 17 * 32);
    }

    #[test]
    fn empty_witness_size_is_zero() {
        let w = IpaWitness::new();
        assert_eq!(w.size_bytes(), 0);
    }

    #[test]
    fn witness_size_is_sum_of_openings() {
        let openings = vec![
            IpaOpening {
                commitment: GroupElement([0; 32]),
                domain_index: 0,
                claimed_value: [0; 32],
                ipa_proof_bytes: vec![0u8; 17 * 32],
            },
            IpaOpening {
                commitment: GroupElement([0; 32]),
                domain_index: 0,
                claimed_value: [0; 32],
                ipa_proof_bytes: vec![0u8; 17 * 32],
            },
        ];
        let witness = IpaWitness { openings };
        assert_eq!(witness.size_bytes(), 2 * (32 + 1 + 32 + 17 * 32));
    }

    #[test]
    fn commitment_scheme_default_is_hash_based() {
        assert_eq!(
            CommitmentSchemeKind::default(),
            CommitmentSchemeKind::HashBased
        );
    }
}
