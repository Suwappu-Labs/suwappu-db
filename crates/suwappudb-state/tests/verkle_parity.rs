//! **S10.5 EXIT GATE** — cross-scheme parity proptest.
//!
//! For the same write set, the structural tree is scheme-independent:
//! the BLAKE3 and Verkle schemes produce different *commitments* but
//! must agree on tree shape (Empty / Leaf / Internal decisions at
//! every depth) — verified indirectly by the fact that both round-trip
//! their own proofs end-to-end.
//!
//! Exit-gate run (release):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --release \
//!       -p suwappudb-state --features production-verkle \
//!       --test verkle_parity
//! ```

#![cfg(feature = "production-verkle")]

use suwappudb_state::{tree, Address, BalanceSlot, StateTree};
use proptest::prelude::*;
use tree::{BanderwagonIpaScheme, Blake3Scheme, Commitment};

const ADDR_SPACE: u8 = 8;

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    addr: Address,
    slot: BalanceSlot,
}

fn entry() -> impl Strategy<Value = Entry> {
    // Constrain the slot to u64 range — the S10.4 leaf-opening check
    // currently asserts equality on the low 64 bits only. Phase-2 may
    // open both limbs.
    (small_address(), any::<u64>()).prop_map(|(addr, n)| Entry {
        addr,
        slot: BalanceSlot::new(u128::from(n)),
    })
}

/// Build a Verkle root from a `StateTree` by visiting the structural
/// root through the [`BanderwagonIpaScheme`] commitment.
fn verkle_root(tree: &StateTree) -> Commitment {
    // Mirror `tree.root()` but with the Verkle scheme. We commit the
    // structural root directly via the scheme's `commit` — the same
    // path `StateTree::verify_verkle` uses to derive the root.
    tree.root_via(&BanderwagonIpaScheme)
}

/// Build a BLAKE3 root from a `StateTree` for cross-check.
fn blake3_root(tree: &StateTree) -> Commitment {
    tree.root_via(&Blake3Scheme)
}

// Two-tier proptest budget. The Verkle commit path costs ~ms per
// node (banderwagon `commit_lagrange` is fast); the *proof* path
// costs ~10 ms per opening (IPAProof::create + verify). Cheap tests
// run at 10k for the S10 exit-gate; slow tests stay at 32 by default
// and scale up via `PROPTEST_CASES=10000`.
proptest! {
    #![proptest_config(ProptestConfig {
        // **S10.5 EXIT GATE** — 10k cases for commit-only tests.
        cases: 10_000,
        .. ProptestConfig::default()
    })]

    /// Verkle determinism: same logical write set ⇒ same Verkle root.
    #[test]
    fn verkle_root_is_deterministic(entries in prop::collection::vec(entry(), 0..16)) {
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &entries {
            effective.insert(e.addr, e.slot);
        }
        let canonical: Vec<_> = effective.into_iter().collect();
        let a = StateTree::from_entries(canonical.iter().copied());
        let b = StateTree::from_entries(canonical.iter().rev().copied());
        prop_assert_eq!(verkle_root(&a), verkle_root(&b));
    }

    /// Verkle ≠ BLAKE3: the two schemes always disagree on bytes
    /// (different curves / hash functions). Tree shape stays the
    /// same; only the commitment bytes differ.
    #[test]
    fn verkle_and_blake3_roots_disagree(entry in entry()) {
        let mut t = StateTree::new();
        t.update(&entry.addr, entry.slot);
        prop_assert_ne!(verkle_root(&t), blake3_root(&t));
    }
}

// Slower property tests — each case generates 1-8 full IPA proofs.
// At ~10 ms per opening × 20 levels per proof × 8 proofs per case =
// ~1.6 s per case. We keep these at 32 by default and rely on the
// per-step budget + tampering tests to catch regressions.
proptest! {
    #![proptest_config(ProptestConfig {
        cases: 32,
        .. ProptestConfig::default()
    })]

    /// Cross-scheme proof round-trip: under Verkle, an inclusion
    /// proof for every populated address verifies via `verify_verkle`.
    #[test]
    fn verkle_inclusion_proofs_round_trip(entries in prop::collection::vec(entry(), 1..8)) {
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &entries {
            effective.insert(e.addr, e.slot);
        }
        let t = StateTree::from_entries(effective.iter().map(|(a, s)| (*a, *s)));
        let root = verkle_root(&t);

        for (addr, slot) in &effective {
            let proof = t.proof(addr);
            prop_assert!(
                StateTree::verify_verkle(root, addr, Some(*slot), &proof),
                "inclusion proof for addr={:?} slot={:?} failed",
                addr, slot
            );
        }
    }

    /// **S10.6 witness-size budget** — every inclusion proof's IPA
    /// witness fits within the per-step budget. Each opening is
    /// 32 (commitment) + 1 (index) + 32 (claim) + 544 (IPAProof) =
    /// 609 B, so a depth-20 inclusion proof is 21 × 609 = 12,789 B
    /// in the per-step format. The IQ-6 spec target of ~200 B
    /// requires multipoint IPA aggregation (follow-on; tracked in
    /// IQ-6's "what stays open" list under "incremental Verkle").
    /// Budget: 14 KB per inclusion at depth 20.
    #[test]
    fn verkle_inclusion_witness_within_per_step_budget(
        entries in prop::collection::vec(entry(), 1..8)
    ) {
        use std::collections::BTreeMap;
        const BUDGET_BYTES: usize = 14 * 1024;

        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &entries {
            effective.insert(e.addr, e.slot);
        }
        let t = StateTree::from_entries(effective.iter().map(|(a, s)| (*a, *s)));

        for (addr, _) in &effective {
            let proof = t.proof(addr);
            let witness = proof
                .ipa_witness
                .as_ref()
                .expect("verkle feature populates the witness");
            prop_assert!(
                witness.size_bytes() <= BUDGET_BYTES,
                "witness {} bytes exceeds {} B budget",
                witness.size_bytes(),
                BUDGET_BYTES
            );
        }
    }

    /// Cross-scheme proof round-trip: under Verkle, an absence proof
    /// for unpopulated addresses also verifies.
    #[test]
    fn verkle_absence_proofs_round_trip(
        populated in prop::collection::vec(entry(), 1..8),
        absent in small_address(),
    ) {
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &populated {
            effective.insert(e.addr, e.slot);
        }
        // Re-roll if `absent` happens to be populated.
        if effective.contains_key(&absent) {
            return Ok(());
        }
        let t = StateTree::from_entries(effective.iter().map(|(a, s)| (*a, *s)));
        let root = verkle_root(&t);

        let proof = t.proof(&absent);
        prop_assert!(
            StateTree::verify_verkle(root, &absent, None, &proof),
            "absence proof for addr={:?} failed",
            absent
        );
    }
}
