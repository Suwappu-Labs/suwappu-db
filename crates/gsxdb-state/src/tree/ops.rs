//! `StateTree` build, update, root, proof, verify.

use super::commit::commit_node;
use super::types::{Commitment, Node, Proof, ProofStep};
use crate::{Address, BalanceSlot};
use std::collections::BTreeMap;

/// 256-ary trie over `Address → BalanceSlot`. Cheap to construct.
#[derive(Debug, Clone, Default)]
pub struct StateTree {
    root: Node,
}

impl StateTree {
    /// Empty tree. `root()` returns the empty commitment.
    #[must_use]
    pub fn new() -> Self {
        Self { root: Node::Empty }
    }

    /// Build a tree from an iterator of `(addr, slot)` pairs. Pairs
    /// can arrive in any order; the tree is canonical regardless.
    pub fn from_entries<I: IntoIterator<Item = (Address, BalanceSlot)>>(entries: I) -> Self {
        let mut tree = Self::new();
        for (addr, slot) in entries {
            tree.update(&addr, slot);
        }
        tree
    }

    /// Build a tree from the full state snapshot. Equivalent to
    /// `from_entries(state.entries())`.
    #[must_use]
    pub fn from_state(state: &crate::State) -> Self {
        Self::from_entries(state.entries())
    }

    /// Insert or replace the leaf at `addr`.
    pub fn update(&mut self, addr: &Address, slot: BalanceSlot) {
        update_path(&mut self.root, &addr.0, 0, slot);
    }

    /// Lookup the slot at `addr`, if any.
    #[must_use]
    pub fn get(&self, addr: &Address) -> Option<BalanceSlot> {
        get_path(&self.root, &addr.0, 0)
    }

    /// Root commitment. Recomputes from scratch each call (phase-1
    /// simplification). S6.5 / S8 introduces caching with explicit
    /// dirty marks.
    #[must_use]
    pub fn root(&self) -> Commitment {
        commit_node(&self.root)
    }

    /// Inclusion / non-inclusion proof for `addr`.
    ///
    /// The returned `Proof` includes the slot (if present) and one
    /// step per address byte. Verifiable via [`StateTree::verify`].
    ///
    /// In Phase 1, the IPA witness is `None`. In S10+, it's populated
    /// by the Verkle prover for witness compression.
    #[must_use]
    pub fn proof(&self, addr: &Address) -> Proof {
        let mut path = Vec::with_capacity(addr.0.len());
        let slot = collect_proof(&self.root, &addr.0, 0, &mut path);
        Proof {
            path,
            slot,
            #[cfg(feature = "production-verkle")]
            ipa_witness: None,
        }
    }

    /// Verify a proof against a known root.
    ///
    /// `slot_opt` is the claimed slot value (`None` means "claim:
    /// addr is not in the tree"). The proof's own `slot` field is
    /// authoritative; we cross-check `slot_opt` matches.
    ///
    /// # Proof shape
    ///
    /// - **Inclusion:** `proof.path.len() == 20`, `proof.slot = Some(_)`.
    ///   The bottom step's byte contributes the leaf commitment.
    /// - **Absence with early termination:** `proof.path.len() < 20`,
    ///   `proof.slot = None`. The bottom step's byte has no child at
    ///   that depth in the actual tree — we exclude it from the
    ///   reconstructed parent commitment.
    /// - **Absence in empty tree:** `proof.path.len() == 0`,
    ///   `proof.slot = None`. Verifies iff `root` is the empty
    ///   commitment.
    #[must_use]
    pub fn verify(
        root: Commitment,
        addr: &Address,
        slot_opt: Option<BalanceSlot>,
        proof: &Proof,
    ) -> bool {
        use super::commit::empty_commitment;
        use blake3::Hasher;
        const TAG_INTERNAL: &[u8] = b"GSXDB-TREE/INT__";

        if proof.slot != slot_opt {
            return false;
        }
        // Path bytes must match the address prefix.
        if proof.path.len() > addr.0.len() {
            return false;
        }
        for (i, step) in proof.path.iter().enumerate() {
            if step.byte != addr.0[i] {
                return false;
            }
        }
        // Inclusion: path must reach full depth.
        if proof.slot.is_some() && proof.path.len() != addr.0.len() {
            return false;
        }

        // Empty-tree absence: no path, root must be empty.
        if proof.path.is_empty() {
            return proof.slot.is_none() && root == empty_commitment();
        }

        // Bottom-up reconstruction.
        let mut current = proof.slot.map(|slot| commit_node(&Node::Leaf(slot)));

        for step in proof.path.iter().rev() {
            let mut combined = step.siblings.clone();
            if let Some(c) = current {
                combined.insert(step.byte, c);
            }
            let mut h = Hasher::new();
            h.update(TAG_INTERNAL);
            for (b, c) in &combined {
                h.update(&[*b]);
                h.update(&c.0);
            }
            let mut out = [0u8; 32];
            out.copy_from_slice(h.finalize().as_bytes());
            current = Some(Commitment(out));
        }

        current == Some(root)
    }
}

fn update_path(node: &mut Node, addr_bytes: &[u8], depth: usize, slot: BalanceSlot) {
    if depth == addr_bytes.len() {
        // Reached leaf depth.
        *node = Node::Leaf(slot);
        return;
    }
    let byte = addr_bytes[depth];
    // Promote Empty / Leaf at this level into an Internal so we can
    // descend. A Leaf at non-leaf depth is impossible by construction
    // (paths are fixed length 20), but defensive: replace with Empty
    // children map.
    if matches!(node, Node::Empty | Node::Leaf(_)) {
        *node = Node::Internal(BTreeMap::new());
    }
    let Node::Internal(children) = node else {
        unreachable!("just promoted to Internal");
    };
    let entry = children
        .entry(byte)
        .or_insert_with(|| Box::new(Node::Empty));
    update_path(entry, addr_bytes, depth + 1, slot);
}

fn get_path(node: &Node, addr_bytes: &[u8], depth: usize) -> Option<BalanceSlot> {
    if depth == addr_bytes.len() {
        return match node {
            Node::Leaf(slot) => Some(*slot),
            _ => None,
        };
    }
    let byte = addr_bytes[depth];
    match node {
        Node::Internal(children) => children
            .get(&byte)
            .and_then(|child| get_path(child, addr_bytes, depth + 1)),
        _ => None,
    }
}

fn collect_proof(
    node: &Node,
    addr_bytes: &[u8],
    depth: usize,
    out: &mut Vec<ProofStep>,
) -> Option<BalanceSlot> {
    if depth == addr_bytes.len() {
        return match node {
            Node::Leaf(slot) => Some(*slot),
            _ => None,
        };
    }
    let byte = addr_bytes[depth];
    match node {
        Node::Internal(children) => {
            // Push a step for this internal node and recurse.
            let mut siblings: BTreeMap<u8, Commitment> = BTreeMap::new();
            for (k, child) in children {
                if *k != byte {
                    siblings.insert(*k, commit_node(child));
                }
            }
            out.push(ProofStep { byte, siblings });
            match children.get(&byte) {
                Some(child) => collect_proof(child, addr_bytes, depth + 1, out),
                None => None,
            }
        }
        // Empty (or unexpectedly Leaf at non-leaf depth) — terminate
        // without pushing a step. The proof captures only the path
        // through real internal nodes.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn empty_tree_lookup_returns_none() {
        let t = StateTree::new();
        assert_eq!(t.get(&addr(1)), None);
    }

    #[test]
    fn update_then_get_round_trips() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(42));
        assert_eq!(t.get(&addr(1)), Some(BalanceSlot::new(42)));
        assert_eq!(t.get(&addr(2)), None);
    }

    #[test]
    fn root_is_deterministic_for_same_state() {
        let mut a = StateTree::new();
        a.update(&addr(1), BalanceSlot::new(10));
        a.update(&addr(2), BalanceSlot::new(20));

        let mut b = StateTree::new();
        b.update(&addr(2), BalanceSlot::new(20));
        b.update(&addr(1), BalanceSlot::new(10));

        assert_eq!(a.root(), b.root());
    }

    #[test]
    fn root_changes_when_state_changes() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(10));
        let r1 = t.root();
        t.update(&addr(1), BalanceSlot::new(11));
        let r2 = t.root();
        assert_ne!(r1, r2);
    }

    #[test]
    fn root_distinguishes_addresses() {
        let mut a = StateTree::new();
        a.update(&addr(1), BalanceSlot::new(42));

        let mut b = StateTree::new();
        b.update(&addr(2), BalanceSlot::new(42));

        assert_ne!(a.root(), b.root());
    }

    #[test]
    fn empty_tree_root_is_stable() {
        let a = StateTree::new();
        let b = StateTree::new();
        assert_eq!(a.root(), b.root());
    }

    #[test]
    fn proof_of_inclusion_verifies() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(42));
        t.update(&addr(2), BalanceSlot::new(99));
        t.update(&addr(3), BalanceSlot::new(0));

        let p = t.proof(&addr(1));
        assert_eq!(p.slot, Some(BalanceSlot::new(42)));
        assert!(StateTree::verify(
            t.root(),
            &addr(1),
            Some(BalanceSlot::new(42)),
            &p
        ));
    }

    #[test]
    fn proof_rejects_tampered_slot() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(42));

        let p = t.proof(&addr(1));
        assert!(!StateTree::verify(
            t.root(),
            &addr(1),
            Some(BalanceSlot::new(43)),
            &p
        ));
    }

    #[test]
    fn proof_of_absence_verifies() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(42));

        let p = t.proof(&addr(99));
        assert_eq!(p.slot, None);
        assert!(StateTree::verify(t.root(), &addr(99), None, &p));
    }

    #[test]
    fn proof_rejects_false_membership_claim() {
        let mut t = StateTree::new();
        t.update(&addr(1), BalanceSlot::new(42));

        let p = t.proof(&addr(99));
        // Claim addr(99) is in the tree with some slot — must reject.
        assert!(!StateTree::verify(
            t.root(),
            &addr(99),
            Some(BalanceSlot::new(0)),
            &p
        ));
    }

    #[test]
    fn from_entries_matches_sequential_updates() {
        let mut seq = StateTree::new();
        seq.update(&addr(1), BalanceSlot::new(1));
        seq.update(&addr(2), BalanceSlot::new(2));
        seq.update(&addr(3), BalanceSlot::new(3));

        let bulk = StateTree::from_entries(vec![
            (addr(3), BalanceSlot::new(3)),
            (addr(1), BalanceSlot::new(1)),
            (addr(2), BalanceSlot::new(2)),
        ]);

        assert_eq!(seq.root(), bulk.root());
    }

    #[test]
    fn many_addresses_all_verifiable() {
        let mut t = StateTree::new();
        for n in 0u8..32 {
            t.update(&addr(n), BalanceSlot::new(u128::from(n) * 100));
        }
        let r = t.root();
        for n in 0u8..32 {
            let p = t.proof(&addr(n));
            assert!(
                StateTree::verify(r, &addr(n), Some(BalanceSlot::new(u128::from(n) * 100)), &p),
                "addr {n} did not verify"
            );
        }
    }
}
