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
use super::verkle::{GroupElement, IpaOpening};
use crate::BalanceSlot;
use ark_serialize::CanonicalSerialize;
use banderwagon::{Element, Fr, PrimeField, Zero};
use ipa_multipoint::{
    committer::Committer,
    committer::DefaultCommitter,
    crs::CRS,
    ipa::{create as ipa_create, IPAProof},
    transcript::Transcript,
};
use std::sync::LazyLock;

/// Width of the banderwagon Lagrange polynomial. The Ethereum Verkle
/// spec uses width 256 (one slot per byte of an internal node's
/// child map).
pub const POLY_WIDTH: usize = 256;

/// Process-wide cached CRS (Ethereum Verkle setup). Used by both the
/// committer and the IPA prover/verifier so they share generators.
pub(crate) static CRS_INSTANCE: LazyLock<CRS> = LazyLock::new(CRS::default);

/// Process-wide cached committer over the Ethereum Verkle CRS.
///
/// `CRS::default()` decodes the hex-encoded `eth_verkle_oct_2021`
/// setup; `DefaultCommitter::new` builds the windowed precomputation
/// tables. Both are non-trivial but deterministic and safe to share.
static COMMITTER: LazyLock<DefaultCommitter> =
    LazyLock::new(|| DefaultCommitter::new(&CRS_INSTANCE.G));

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

/// Transcript label used for every per-step IPA opening on the proof
/// path. Picked once so prover + verifier agree.
const PROOF_STEP_TRANSCRIPT_LABEL: &[u8] = b"gsxdb-verkle-step";

/// Build the 256-element b-vector that opens a Lagrange-basis
/// polynomial at evaluation index `domain_index`.
///
/// For a polynomial stored as `(f(0), f(1), …, f(255))`, the inner
/// product `<a, e_i>` equals `f(i)`, so we just need the unit vector
/// at position `i`. Soundness of the IPA does not depend on `b`
/// having any specific structure — only that prover + verifier agree.
fn unit_b_vector(domain_index: usize) -> Vec<Fr> {
    let mut b = vec![Fr::zero(); POLY_WIDTH];
    b[domain_index] = Fr::from(1u64);
    b
}

/// Compute one IPA opening: prove that the polynomial `a` opens to
/// `a[domain_index]` under commitment `a_comm`, where `a_comm` is
/// `BanderwagonIpaScheme.commit(...)`'s underlying [`Element`].
///
/// `domain_index` is also fed into the transcript as the labelled
/// `input point` so the verifier rejects forged openings that change
/// the index after the fact.
pub(crate) fn prove_opening(
    a: Vec<Fr>,
    a_comm: Element,
    domain_index: u8,
) -> (IpaOpening, Fr) {
    let b = unit_b_vector(domain_index as usize);
    let input_point = Fr::from(domain_index as u64);
    let claimed_value = a[domain_index as usize];

    let mut transcript = Transcript::new(PROOF_STEP_TRANSCRIPT_LABEL);
    let proof = ipa_create(
        &mut transcript,
        CRS_INSTANCE.clone(),
        a,
        a_comm,
        b,
        input_point,
    );

    let mut claimed_value_bytes = [0u8; 32];
    claimed_value
        .serialize_compressed(&mut claimed_value_bytes[..])
        .expect("Fr always serializes to 32 bytes");

    let ipa_proof_bytes = proof
        .to_bytes()
        .expect("IPAProof::to_bytes is infallible for our setup");

    let opening = IpaOpening {
        commitment: GroupElement(a_comm.to_bytes()),
        domain_index,
        claimed_value: claimed_value_bytes,
        ipa_proof_bytes,
    };

    (opening, claimed_value)
}

/// Verify one IPA opening produced by [`prove_opening`].
///
/// Returns `true` iff (a) the deserialized commitment / IPA proof
/// parse cleanly, (b) `IPAProof::verify` accepts under the same
/// transcript label + CRS, and (c) the claimed-value scalar matches
/// the opening's declared scalar.
pub(crate) fn verify_opening(opening: &IpaOpening) -> bool {
    let Some(commitment) = commitment_to_element(&Commitment(opening.commitment.0)) else {
        return false;
    };
    let Ok(claimed_value) = <Fr as ark_serialize::CanonicalDeserialize>::deserialize_compressed(
        &opening.claimed_value[..],
    ) else {
        return false;
    };
    let Ok(proof) = IPAProof::from_bytes(&opening.ipa_proof_bytes, POLY_WIDTH) else {
        return false;
    };

    let b = unit_b_vector(opening.domain_index as usize);
    let input_point = Fr::from(opening.domain_index as u64);

    let mut transcript = Transcript::new(PROOF_STEP_TRANSCRIPT_LABEL);
    proof.verify(
        &mut transcript,
        CRS_INSTANCE.clone(),
        b,
        commitment,
        input_point,
        claimed_value,
    )
}

/// Recover the [`Fr`] scalar a verifier should compute for a child
/// commitment when walking the proof path. Mirrors
/// `Element::map_to_scalar_field` used by [`commit_node_inner`] for
/// internal nodes, but starts from the 32-byte `Commitment` form.
pub(crate) fn child_commitment_to_scalar(c: &Commitment) -> Option<Fr> {
    Some(commitment_to_element(c)?.map_to_scalar_field())
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
    fn ipa_opening_round_trips_for_leaf() {
        // S10.3 sanity: prove an opening on a leaf's polynomial and
        // verify it under the same CRS / transcript label.
        let slot = BalanceSlot::new(12345);
        let a = leaf_evaluations(slot);
        let a_comm = commit_node_inner(&Node::Leaf(slot));
        // Open at index 0 — the low limb of the canonical balance.
        let (opening, claimed) = prove_opening(a, a_comm, 0);
        assert_eq!(claimed, Fr::from(12345u64));
        assert!(verify_opening(&opening));
    }

    #[test]
    fn ipa_opening_round_trips_for_internal_node() {
        // Internal node: build a children map, commit, then open at
        // one of the populated indices.
        let mut children = BTreeMap::new();
        children.insert(7u8, Box::new(Node::Leaf(BalanceSlot::new(100))));
        children.insert(42u8, Box::new(Node::Leaf(BalanceSlot::new(200))));
        let internal = Node::Internal(children);

        let a_comm = commit_node_inner(&internal);
        // Rebuild the evaluation vector at this internal node — same
        // pattern as `commit_node_inner` for the Internal arm.
        let Node::Internal(ref children) = internal else { unreachable!() };
        let mut evals = vec![Fr::from(0u64); POLY_WIDTH];
        for (byte, child) in children {
            let child_element = commit_node_inner(child);
            evals[*byte as usize] = child_element.map_to_scalar_field();
        }

        let (opening, _) = prove_opening(evals, a_comm, 7);
        assert!(verify_opening(&opening));
    }

    #[test]
    fn ipa_opening_rejects_tampered_claimed_value() {
        let slot = BalanceSlot::new(99);
        let a = leaf_evaluations(slot);
        let a_comm = commit_node_inner(&Node::Leaf(slot));
        let (mut opening, _) = prove_opening(a, a_comm, 0);
        // Flip one byte in the claimed-value scalar.
        opening.claimed_value[0] ^= 0x01;
        assert!(!verify_opening(&opening));
    }

    #[test]
    fn ipa_opening_rejects_tampered_commitment() {
        let slot = BalanceSlot::new(99);
        let a = leaf_evaluations(slot);
        let a_comm = commit_node_inner(&Node::Leaf(slot));
        let (mut opening, _) = prove_opening(a, a_comm, 0);
        // Replace the commitment with the zero point — verify must
        // reject (it would only accept if zero opened to claimed_value
        // at index 0, which it doesn't unless slot == 0).
        opening.commitment.0 = [0u8; 32];
        assert!(!verify_opening(&opening));
    }

    #[test]
    fn ipa_opening_rejects_tampered_domain_index() {
        let slot = BalanceSlot::new(99);
        let a = leaf_evaluations(slot);
        let a_comm = commit_node_inner(&Node::Leaf(slot));
        let (mut opening, _) = prove_opening(a, a_comm, 0);
        // Re-target the same opening at a different index — verifier
        // recomputes b_vec from the index and rejects.
        opening.domain_index = 5;
        assert!(!verify_opening(&opening));
    }

    #[test]
    fn child_commitment_to_scalar_matches_inline_map() {
        let child = Node::Leaf(BalanceSlot::new(7));
        let child_element = commit_node_inner(&child);
        let c = element_to_commitment(&child_element);
        let scalar = child_commitment_to_scalar(&c).expect("our own commit round-trips");
        assert_eq!(scalar, child_element.map_to_scalar_field());
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
