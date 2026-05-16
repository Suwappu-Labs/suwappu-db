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
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        GroupElement(bytes)
    }

    /// Serialize to 32 bytes.
    pub fn to_bytes(&self) -> [u8; 32] {
        self.0
    }
}

/// Inner Product Argument witness for proof compression.
///
/// Reduces 256 sibling commitments to ~16 group elements (8 L, 8 R)
/// through recursive folding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaWitness {
    /// Left folding elements (one per recursion level, ~8 for 256-ary).
    /// Named `L` to match cryptographic literature.
    pub left: Vec<GroupElement>,
    /// Right folding elements (one per recursion level, ~8 for 256-ary).
    /// Named `right` to match cryptographic literature.
    pub right: Vec<GroupElement>,
    /// Final commitment after all folding
    pub final_commitment: GroupElement,
    /// Final evaluation point (scalar)
    pub final_evaluation: [u8; 32],
}

// Implement L/R accessors for compatibility with cryptographic notation
impl IpaWitness {
    /// Alias for `left` (matches cryptographic notation L_i).
    #[allow(non_snake_case)]
    pub fn L(&self) -> &[GroupElement] {
        &self.left
    }

    /// Alias for `right` (matches cryptographic notation R_i).
    #[allow(non_snake_case)]
    pub fn R(&self) -> &[GroupElement] {
        &self.right
    }
}

impl IpaWitness {
    /// Size in bytes of the witness (for measurement).
    pub fn size_bytes(&self) -> usize {
        32 * (self.left.len() + self.right.len()) + 32 + 32 // L + R + final_commitment + final_evaluation
    }
}

/// Represents whether to use hash-based (Phase 1) or Verkle (S10+) commitments.
///
/// Distinct from [`super::commit::CommitmentScheme`] (the *trait* that
/// the BLAKE3 and banderwagon schemes implement) — this enum is a
/// runtime tag for telemetry / configuration. The trait dispatches
/// the actual commit; this enum names which trait impl is wired in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitmentSchemeKind {
    /// Hash-based (BLAKE3) for Phase 1.
    HashBased,
    /// Verkle with IPA for S10+.
    #[cfg(feature = "production-verkle")]
    Verkle,
}

impl Default for CommitmentSchemeKind {
    fn default() -> Self {
        CommitmentSchemeKind::HashBased
    }
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
    fn ipa_witness_size_estimate() {
        let witness = IpaWitness {
            left: vec![GroupElement([0; 32]); 8],
            right: vec![GroupElement([0; 32]); 8],
            final_commitment: GroupElement([0; 32]),
            final_evaluation: [0; 32],
        };
        // 8 + 8 + 1 + 1 = 18 group elements = 576 bytes
        assert_eq!(witness.size_bytes(), 576);
    }

    #[test]
    fn commitment_scheme_default_is_hash_based() {
        assert_eq!(
            CommitmentSchemeKind::default(),
            CommitmentSchemeKind::HashBased
        );
    }
}
