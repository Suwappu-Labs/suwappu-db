//! **S6 EXIT GATE** — state-tree determinism + proof correctness under load.

use suwappudb_state::{Address, BalanceSlot, StateTree};
use proptest::prelude::*;

const ADDR_SPACE: u8 = 16;

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    addr: Address,
    slot: BalanceSlot,
}

fn entry() -> impl Strategy<Value = Entry> {
    (small_address(), any::<u128>()).prop_map(|(addr, n)| Entry {
        addr,
        slot: BalanceSlot::new(n),
    })
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// Determinism: two trees built from the same logical state (same
    /// final value at every address) have identical roots, regardless
    /// of insert order.
    #[test]
    fn root_is_deterministic(entries in prop::collection::vec(entry(), 0..32)) {
        // Compute the "final value per address" map (last write wins).
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &entries {
            effective.insert(e.addr, e.slot);
        }

        // Build tree A in input order.
        let mut a = StateTree::new();
        for e in &entries {
            a.update(&e.addr, e.slot);
        }

        // Build tree B from the effective map (sorted, no duplicates).
        let b = StateTree::from_entries(effective.into_iter());

        prop_assert_eq!(a.root(), b.root());
    }

    /// Replay equivalence: building the same logical state twice via
    /// independent update sequences produces the same root.
    #[test]
    fn replay_produces_same_root(entries in prop::collection::vec(entry(), 0..32)) {
        let mut a = StateTree::new();
        let mut b = StateTree::new();
        for e in &entries {
            a.update(&e.addr, e.slot);
            b.update(&e.addr, e.slot);
        }
        prop_assert_eq!(a.root(), b.root());
    }

    /// Inclusion: every entry that ended up in the tree (after dedup)
    /// has a proof that verifies against the root.
    #[test]
    fn every_inclusion_proof_verifies(
        entries in prop::collection::vec(entry(), 1..16),
    ) {
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        let mut tree = StateTree::new();
        for e in &entries {
            tree.update(&e.addr, e.slot);
            effective.insert(e.addr, e.slot);
        }
        let root = tree.root();

        for (addr, slot) in &effective {
            let proof = tree.proof(addr);
            prop_assert!(
                StateTree::verify(root, addr, Some(*slot), &proof),
                "inclusion proof failed for {:?}", addr.0
            );
        }
    }

    /// Absence: any address NOT in the effective set produces a proof
    /// that verifies against the root with `slot = None`.
    #[test]
    fn absence_proof_verifies(
        entries in prop::collection::vec(entry(), 0..16),
        absent in (ADDR_SPACE..32).prop_map(|n| Address([n; 20])),
    ) {
        let mut tree = StateTree::new();
        for e in &entries {
            tree.update(&e.addr, e.slot);
        }
        let root = tree.root();

        let proof = tree.proof(&absent);
        prop_assert_eq!(proof.slot, None);
        prop_assert!(
            StateTree::verify(root, &absent, None, &proof),
            "absence proof failed for {:?}", absent.0
        );
    }

    /// Tamper resistance: changing the claimed slot in a verify call
    /// must reject (with overwhelming probability — the only collision
    /// is if the new slot equals the old).
    #[test]
    fn tampered_slot_rejected(
        entries in prop::collection::vec(entry(), 1..16),
        delta in 1u128..1000,
    ) {
        let mut tree = StateTree::new();
        for e in &entries {
            tree.update(&e.addr, e.slot);
        }
        let root = tree.root();

        // Pick the first entry. Flip its slot by `delta`.
        let target = entries[0];
        let proof = tree.proof(&target.addr);
        let actual_slot = proof.slot.expect("proof of inserted addr is inclusion");
        let bumped = BalanceSlot::new(actual_slot.canonical().wrapping_add(delta));

        // Skip the (rare) case where the bumped value happens to equal
        // some prior write to the same address (last-write-wins
        // semantics in `update`). For our size, this is extremely rare.
        prop_assume!(bumped != actual_slot);

        prop_assert!(
            !StateTree::verify(root, &target.addr, Some(bumped), &proof),
            "verify accepted a tampered slot"
        );
    }

    /// **S6 EXIT GATE.** Cross-tree root agreement: two trees built
    /// from the same effective state — regardless of how the writes
    /// were sequenced — produce the same root, AND every address in
    /// that state has a verifying inclusion proof against it.
    #[test]
    fn cross_tree_root_agreement(
        entries in prop::collection::vec(entry(), 0..32),
    ) {
        use std::collections::BTreeMap;
        let mut effective: BTreeMap<Address, BalanceSlot> = BTreeMap::new();
        for e in &entries {
            effective.insert(e.addr, e.slot);
        }

        // Tree built sequentially.
        let mut tree_a = StateTree::new();
        for e in &entries {
            tree_a.update(&e.addr, e.slot);
        }

        // Tree built from the effective map.
        let tree_b = StateTree::from_entries(effective.iter().map(|(a, s)| (*a, *s)));

        // Same root.
        prop_assert_eq!(tree_a.root(), tree_b.root());

        // Every effective address verifies in both.
        for (addr, slot) in &effective {
            let p_a = tree_a.proof(addr);
            let p_b = tree_b.proof(addr);
            prop_assert!(StateTree::verify(tree_a.root(), addr, Some(*slot), &p_a));
            prop_assert!(StateTree::verify(tree_b.root(), addr, Some(*slot), &p_b));
        }
    }
}
