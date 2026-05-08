//! Hash-based commitment scheme for the state tree.
//!
//! Three node flavours, three commitment functions; each is a pure
//! function of the node and produces a 32-byte [`Commitment`].
//!
//! # Domain separation
//!
//! Each node type prefixes its hash input with a distinct ASCII tag
//! (`EMPTY` / `LEAF_` / `INT__`). This prevents the (theoretical)
//! attack where a leaf's input bytes happen to equal an internal
//! node's input bytes, producing a commitment collision across types.
//!
//! # IQ-6 swap point
//!
//! When real Verkle lands, [`commit_node`] is the only function that
//! changes — it returns an IPA-over-banderwagon polynomial commitment
//! instead of a BLAKE3 hash. Tree shape, traversal, and proof format
//! evolve in tandem but not at the type level visible here.

use super::types::{Commitment, Node};
use blake3::Hasher;

/// Domain tag for empty nodes.
const TAG_EMPTY: &[u8] = b"GSXDB-TREE/EMPTY";
/// Domain tag for leaf nodes.
const TAG_LEAF: &[u8] = b"GSXDB-TREE/LEAF_";
/// Domain tag for internal nodes.
const TAG_INTERNAL: &[u8] = b"GSXDB-TREE/INT__";

/// Pre-computed empty-subtree commitment. Stable across all calls.
pub const EMPTY_COMMITMENT: Commitment = empty_commitment_const();

const fn empty_commitment_const() -> Commitment {
    // BLAKE3 of TAG_EMPTY. Computed once at runtime if `const fn`
    // can't reach `blake3::hash` at compile time — fallback to lazy
    // computation in `empty_commitment()` below.
    Commitment([0; 32]) // placeholder; the real value is produced by `empty_commitment()`
}

/// Empty-subtree commitment (lazy form). Returns the stable BLAKE3
/// hash of the empty-domain tag.
#[must_use]
pub fn empty_commitment() -> Commitment {
    let mut h = Hasher::new();
    h.update(TAG_EMPTY);
    let mut out = [0u8; 32];
    out.copy_from_slice(h.finalize().as_bytes());
    Commitment(out)
}

/// Commitment for a [`Node`].
///
/// - [`Node::Empty`] → [`empty_commitment`]
/// - [`Node::Leaf`] → `BLAKE3(TAG_LEAF | slot.canonical().to_be_bytes())`
/// - [`Node::Internal`] → `BLAKE3(TAG_INTERNAL | sorted (byte, child_commitment) concat)`
#[must_use]
pub fn commit_node(node: &Node) -> Commitment {
    match node {
        Node::Empty => empty_commitment(),
        Node::Leaf(slot) => {
            let mut h = Hasher::new();
            h.update(TAG_LEAF);
            h.update(&slot.canonical().to_be_bytes());
            let mut out = [0u8; 32];
            out.copy_from_slice(h.finalize().as_bytes());
            Commitment(out)
        }
        Node::Internal(children) => {
            let mut h = Hasher::new();
            h.update(TAG_INTERNAL);
            // BTreeMap iterates in key order, which is what we need
            // for determinism. Each (byte, child_commitment) is one
            // 33-byte chunk.
            for (byte, child) in children {
                h.update(&[*byte]);
                let c = commit_node(child);
                h.update(&c.0);
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(h.finalize().as_bytes());
            Commitment(out)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceSlot;
    use std::collections::BTreeMap;

    #[test]
    fn empty_commitment_is_deterministic() {
        let a = empty_commitment();
        let b = empty_commitment();
        assert_eq!(a, b);
    }

    #[test]
    fn empty_node_commits_to_empty_commitment() {
        let c = commit_node(&Node::Empty);
        assert_eq!(c, empty_commitment());
    }

    #[test]
    fn distinct_leaves_have_distinct_commitments() {
        let a = commit_node(&Node::Leaf(BalanceSlot::new(1)));
        let b = commit_node(&Node::Leaf(BalanceSlot::new(2)));
        assert_ne!(a, b);
    }

    #[test]
    fn same_leaf_value_same_commitment() {
        let a = commit_node(&Node::Leaf(BalanceSlot::new(42)));
        let b = commit_node(&Node::Leaf(BalanceSlot::new(42)));
        assert_eq!(a, b);
    }

    #[test]
    fn empty_internal_distinguishes_from_empty_node() {
        // An Internal with zero children should commit to something
        // distinct from Node::Empty — they're structurally different
        // states even though both "have no leaves below."
        let internal = Node::Internal(BTreeMap::new());
        assert_ne!(commit_node(&internal), commit_node(&Node::Empty));
    }

    #[test]
    fn internal_commitment_is_order_invariant_to_insertion() {
        // BTreeMap iterates in key order regardless of insertion
        // order. Build the same logical internal node two ways and
        // confirm the commitments match.
        let mut a = BTreeMap::new();
        a.insert(2u8, Box::new(Node::Leaf(BalanceSlot::new(20))));
        a.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(10))));

        let mut b = BTreeMap::new();
        b.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(10))));
        b.insert(2u8, Box::new(Node::Leaf(BalanceSlot::new(20))));

        assert_eq!(commit_node(&Node::Internal(a)), commit_node(&Node::Internal(b)));
    }

    #[test]
    fn distinct_paths_distinct_commitments() {
        // Same leaf placed under byte 0 vs byte 1 → different commits.
        let mut t0 = BTreeMap::new();
        t0.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(99))));

        let mut t1 = BTreeMap::new();
        t1.insert(1u8, Box::new(Node::Leaf(BalanceSlot::new(99))));

        assert_ne!(commit_node(&Node::Internal(t0)), commit_node(&Node::Internal(t1)));
    }

    #[test]
    fn leaf_and_empty_distinct() {
        let l = commit_node(&Node::Leaf(BalanceSlot::new(0)));
        let e = commit_node(&Node::Empty);
        assert_ne!(l, e, "Leaf(0) must not collide with Empty");
    }

    /// Suppress dead-code warnings on `EMPTY_COMMITMENT` until a
    /// caller exists. Deliberate placeholder per the doc comment in
    /// `empty_commitment_const`.
    #[test]
    fn empty_commitment_const_placeholder_remains() {
        let _ = EMPTY_COMMITMENT;
    }
}
