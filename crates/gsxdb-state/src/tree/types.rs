//! State-tree data types.

use crate::BalanceSlot;
use std::collections::BTreeMap;

#[cfg(feature = "production-verkle")]
use super::verkle::IpaWitness;

/// 32-byte commitment. Output of [`super::commit::commit_node`].
///
/// Newtype to keep "this is a tree commitment" distinct from "this is
/// some other 32-byte hash" at the type level.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Commitment(pub [u8; 32]);

impl std::fmt::Debug for Commitment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Commitment(0x")?;
        for b in &self.0 {
            write!(f, "{b:02x}")?;
        }
        write!(f, ")")
    }
}

/// 256-ary trie node.
///
/// `Empty` is structurally distinct from "internal node with zero
/// children" so the empty commitment is well-defined and stable
/// regardless of how the trie was assembled.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Node {
    /// No content. Both root of an empty tree and any unallocated
    /// subtree.
    #[default]
    Empty,
    /// Leaf at a 20-byte path. Holds the canonical balance slot.
    Leaf(BalanceSlot),
    /// Internal node. `BTreeMap` for deterministic iteration order
    /// (child commitments are concatenated in key order).
    Internal(BTreeMap<u8, Box<Node>>),
}

/// One step of an inclusion / non-inclusion proof.
///
/// `step.byte` is the address byte at the step's depth; `step.siblings`
/// is the map of (byte → sibling commitment) for every other byte that
/// has a non-empty subtree at this depth. Reconstructing the parent
/// commitment from the proven child + siblings yields the parent's
/// commitment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProofStep {
    /// Address byte at this depth.
    pub byte: u8,
    /// Sibling commitments at this depth, keyed by their byte. Does
    /// NOT include `byte` (that's reconstructed from the level below).
    pub siblings: BTreeMap<u8, Commitment>,
}

/// Inclusion or non-inclusion proof for an address.
///
/// Phase 1 (S1–S8): Contains only the Merkle path and slot.
/// Phase 2 (S10+): Optionally includes an IPA witness for Verkle compression.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proof {
    /// One step per byte of the address (depth 20 for 20-byte addresses).
    /// Ordered root → leaf.
    pub path: Vec<ProofStep>,
    /// The slot present at the address, or `None` if proving non-inclusion.
    pub slot: Option<BalanceSlot>,
    /// Verkle IPA witness for compressed verification (S10+).
    /// `None` in Phase 1; filled by IPA prover in S10+.
    #[cfg(feature = "production-verkle")]
    pub ipa_witness: Option<IpaWitness>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_node_is_default() {
        let n = Node::default();
        assert!(matches!(n, Node::Empty));
    }

    #[test]
    fn commitment_debug_is_hex() {
        let c = Commitment([0xab; 32]);
        let s = format!("{c:?}");
        assert!(s.contains("ab"));
        assert!(s.starts_with("Commitment(0x"));
    }

    #[test]
    fn distinct_commitments_distinct() {
        let a = Commitment([0; 32]);
        let mut b_bytes = [0u8; 32];
        b_bytes[0] = 1;
        let b = Commitment(b_bytes);
        assert_ne!(a, b);
    }
}
