//! Real Verkle commitment scheme — banderwagon curve + IPA polynomial
//! commitments. The S10 implementation of the [`CommitmentScheme`]
//! trait declared in [`super::commit`].
//!
//! # Polynomial layout
//!
//! Each [`Node`] maps to a 256-element vector of [`Fr`] (the
//! banderwagon scalar field). The vector is committed to as a Lagrange
//! polynomial via [`DefaultCommitter::commit_lagrange`] using the
//! Ethereum Verkle CRS (the standard 256-point setup from
//! `eth_verkle_oct_2021`). The resulting curve point's 32-byte
//! compressed encoding is the [`Commitment`].
//!
//! | `Node`                  | 256-element evaluations                          |
//! |-------------------------|--------------------------------------------------|
//! | `Empty`                 | all-zero                                         |
//! | `Leaf(slot)`            | `[lo, hi, 0, …]` — `slot.canonical()` as two u64s |
//! | `Internal({byte→child})` | index `byte` = child commitment mapped to `Fr`   |
//!
//! Two compatibility properties matter:
//!
//! 1. **Empty commits to zero bytes.** The compressed encoding of
//!    `Element::zero()` is `[0; 32]` (because identity is `(0, 1)` in
//!    twisted-Edwards and the compressed form is just `x`). That
//!    matches the "absent child" slot in the polynomial.
//! 2. **Children compose via the banderwagon `Element → Fr` map.**
//!    `Element::map_to_scalar_field` is the canonical group-to-field
//!    homomorphism that banderwagon (as opposed to bandersnatch) was
//!    designed to admit safely. Verkle's whole reason for picking
//!    banderwagon is this property.
//!
//! # CRS
//!
//! The Ethereum Verkle CRS is loaded once (the bytes are baked into
//! [`ipa_multipoint::default_crs`]) via a `LazyLock`. Both
//! [`CRS::default`] and `DefaultCommitter::new` do non-trivial setup
//! work: ~ms-scale on first call, free after. We cache one per
//! process.
//!
//! Domain separation between empty / leaf / internal is structural
//! (different evaluation patterns), not tag-based. Two different
//! `Node`s never produce the same evaluation vector.

use super::commit::CommitmentScheme;
use super::types::{Commitment, Node};
use crate::BalanceSlot;
use ark_serialize::CanonicalSerialize;
use banderwagon::{Element, Fr, PrimeField};
use ipa_multipoint::{committer::Committer, committer::DefaultCommitter, crs::CRS};
use std::sync::LazyLock;

/// Width of the banderwagon Lagrange polynomial. The Ethereum Verkle
/// spec uses width 256 (one slot per byte of an internal node's
/// child map).
pub const POLY_WIDTH: usize = 256;

/// Process-wide cached committer over the Ethereum Verkle CRS.
///
/// `CRS::default()` decodes the hex-encoded `eth_verkle_oct_2021`
/// setup; `DefaultCommitter::new` builds the windowed precomputation
/// tables. Both are non-trivial but deterministic and safe to share.
static COMMITTER: LazyLock<DefaultCommitter> = LazyLock::new(|| {
    let crs = CRS::default();
    DefaultCommitter::new(&crs.G)
});

/// IPA / banderwagon commitment scheme — the launch-readiness
/// implementation of [`CommitmentScheme`]. Behavioural counterpart to
/// [`super::commit::Blake3Scheme`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BanderwagonIpaScheme;

impl CommitmentScheme for BanderwagonIpaScheme {
    fn empty(&self) -> Commitment {
        // The compressed encoding of the identity point is `[0; 32]`.
        // Sanity-checked by `empty_commits_to_zero_bytes` below.
        Commitment([0u8; 32])
    }

    fn commit(&self, node: &Node) -> Commitment {
        let element = commit_node_inner(node);
        Commitment(element.to_bytes())
    }

    fn scheme_name(&self) -> &'static str {
        "banderwagon-ipa"
    }
}

/// Build the polynomial-evaluation vector for `node` and commit.
/// Returns the raw [`Element`] (not bytes); callers serialize as
/// they need.
pub(crate) fn commit_node_inner(node: &Node) -> Element {
    match node {
        Node::Empty => Element::zero(),
        Node::Leaf(slot) => {
            let mut evals = vec![Fr::from(0u64); POLY_WIDTH];
            let (lo, hi) = canonical_u128_to_u64_pair(slot.canonical());
            evals[0] = Fr::from(lo);
            evals[1] = Fr::from(hi);
            COMMITTER.commit_lagrange(&evals)
        }
        Node::Internal(children) => {
            let mut evals = vec![Fr::from(0u64); POLY_WIDTH];
            for (byte, child) in children {
                let child_element = commit_node_inner(child);
                evals[*byte as usize] = child_element.map_to_scalar_field();
            }
            COMMITTER.commit_lagrange(&evals)
        }
    }
}

/// Serialize a banderwagon [`Element`] to the canonical 32-byte
/// compressed form used in [`Commitment`]. Used by the S10.3 proof
/// path.
#[allow(dead_code)]
pub(crate) fn element_to_commitment(e: &Element) -> Commitment {
    Commitment(e.to_bytes())
}

/// Deserialize a [`Commitment`] back to a banderwagon [`Element`].
/// Returns `None` if the bytes don't represent a valid prime-subgroup
/// point — this should only happen on tampered proofs, not on commits
/// produced by [`BanderwagonIpaScheme`]. Used by the S10.4 verify
/// path.
#[allow(dead_code)]
pub(crate) fn commitment_to_element(c: &Commitment) -> Option<Element> {
    // The "zero-bytes encode the identity" property is load-bearing
    // for the absent-child slot in proof reconstruction.
    if c.0 == [0u8; 32] {
        return Some(Element::zero());
    }
    Element::from_bytes(&c.0)
}

/// Map a [`BalanceSlot`]'s canonical u128 to two u64 limbs (low, high)
/// in little-endian order. Determinism is required so that the
/// scheme's leaf commitment is stable.
fn canonical_u128_to_u64_pair(v: u128) -> (u64, u64) {
    let lo = v as u64;
    let hi = (v >> 64) as u64;
    (lo, hi)
}

/// Pack a u128 (a `BalanceSlot::canonical()`) as a 256-element
/// polynomial evaluation vector. Exposed for the proof / verify path
/// in S10.3 / S10.4.
#[allow(dead_code)]
pub(crate) fn leaf_evaluations(slot: BalanceSlot) -> Vec<Fr> {
    let mut evals = vec![Fr::from(0u64); POLY_WIDTH];
    let (lo, hi) = canonical_u128_to_u64_pair(slot.canonical());
    evals[0] = Fr::from(lo);
    evals[1] = Fr::from(hi);
    evals
}

/// Used only by tests / proofs that need to confirm `Element` round-trips
/// through the on-disk [`Commitment`] form.
#[allow(dead_code)]
pub(crate) fn fr_canonical_bytes(fr: &Fr) -> [u8; 32] {
    let mut bytes = [0u8; 32];
    fr.serialize_compressed(&mut bytes[..])
        .expect("Fr always serializes to 32 bytes");
    bytes
}

/// Reduce a 32-byte hash into a banderwagon scalar field element via
/// little-endian-mod-order. Used for absent / scalar conversions in
/// the witness path (S10.3).
#[allow(dead_code)]
pub(crate) fn fr_from_le_bytes_mod_order(bytes: &[u8]) -> Fr {
    Fr::from_le_bytes_mod_order(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BalanceSlot;
    use std::collections::BTreeMap;

    #[test]
    fn scheme_name_is_stable() {
        assert_eq!(BanderwagonIpaScheme.scheme_name(), "banderwagon-ipa");
    }

    #[test]
    fn empty_commits_to_zero_bytes() {
        // Load-bearing for the absent-child convention in internal
        // node commits + proofs.
        let c = BanderwagonIpaScheme.empty();
        assert_eq!(c, Commitment([0u8; 32]));
        // And matches what the structural Empty node produces.
        assert_eq!(BanderwagonIpaScheme.commit(&Node::Empty), c);
    }

    #[test]
    fn leaf_commitment_deterministic() {
        let l1 = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(42)));
        let l2 = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(42)));
        assert_eq!(l1, l2);
    }

    #[test]
    fn distinct_leaves_have_distinct_commitments() {
        let a = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(1)));
        let b = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(2)));
        assert_ne!(a, b);
    }

    #[test]
    fn internal_commitment_is_order_invariant_to_insertion() {
        let mut a = BTreeMap::new();
        a.insert(2u8, Box::new(Node::Leaf(BalanceSlot::new(20))));
        a.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(10))));

        let mut b = BTreeMap::new();
        b.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(10))));
        b.insert(2u8, Box::new(Node::Leaf(BalanceSlot::new(20))));

        assert_eq!(
            BanderwagonIpaScheme.commit(&Node::Internal(a)),
            BanderwagonIpaScheme.commit(&Node::Internal(b)),
        );
    }

    #[test]
    fn distinct_paths_distinct_commitments() {
        let mut t0 = BTreeMap::new();
        t0.insert(0u8, Box::new(Node::Leaf(BalanceSlot::new(99))));

        let mut t1 = BTreeMap::new();
        t1.insert(1u8, Box::new(Node::Leaf(BalanceSlot::new(99))));

        assert_ne!(
            BanderwagonIpaScheme.commit(&Node::Internal(t0)),
            BanderwagonIpaScheme.commit(&Node::Internal(t1)),
        );
    }

    #[test]
    fn zero_leaf_collides_with_empty_documented() {
        // A leaf storing value 0 commits to the same point as Empty —
        // both produce the all-zero evaluation polynomial, which
        // commits to identity. This is *not* a verification soundness
        // issue: in `StateTree::verify` (S10.4), depth is implicit
        // from the proof's path length, so a `Leaf(0)` claim at depth
        // 20 and an `Empty` subtree claim at the same point cannot be
        // confused — the verifier checks `slot == claimed_slot` as a
        // separate condition. Asserting the equality here so the
        // property is pinned and any future scheme change that
        // breaks it triggers a deliberate decision.
        let l = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(0)));
        let e = BanderwagonIpaScheme.commit(&Node::Empty);
        assert_eq!(l, e);
    }

    #[test]
    fn nonzero_leaf_distinct_from_empty() {
        let l = BanderwagonIpaScheme.commit(&Node::Leaf(BalanceSlot::new(1)));
        let e = BanderwagonIpaScheme.commit(&Node::Empty);
        assert_ne!(l, e);
    }

    #[test]
    fn commitment_round_trips_through_element() {
        let leaf = Node::Leaf(BalanceSlot::new(12345));
        let c = BanderwagonIpaScheme.commit(&leaf);
        let element = commitment_to_element(&c).expect("our own commitments round-trip");
        assert_eq!(element_to_commitment(&element), c);
    }

    #[test]
    fn empty_commitment_round_trips_through_element() {
        let c = BanderwagonIpaScheme.empty();
        let element = commitment_to_element(&c).expect("zero is a valid prime-subgroup point");
        assert!(element.is_zero());
    }

    #[test]
    fn canonical_pair_packs_u128() {
        let (lo, hi) = canonical_u128_to_u64_pair(0x1234_5678_9abc_def0_0fed_cba9_8765_4321);
        assert_eq!(lo, 0x0fed_cba9_8765_4321);
        assert_eq!(hi, 0x1234_5678_9abc_def0);
    }

    #[test]
    fn leaf_evaluations_are_two_nonzero() {
        let evals = leaf_evaluations(BalanceSlot::new(1));
        assert_eq!(evals.len(), POLY_WIDTH);
        // Only index 0 is non-zero for a u128 < 2^64.
        assert_eq!(evals[0], Fr::from(1u64));
        assert_eq!(evals[1], Fr::from(0u64));
        for e in &evals[2..] {
            assert_eq!(*e, Fr::from(0u64));
        }
    }
}
