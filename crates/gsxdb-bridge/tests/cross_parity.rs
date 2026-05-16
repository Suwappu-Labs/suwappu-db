//! **S7 EXIT GATE** — cross-chain parity invariant.
//!
//! For any sequence of `(state_root, height)` dispatches across N
//! chains, the parity check at every height returns Agreed when no
//! tampering occurred, and Disagreed when at least one log is
//! mutated.
//!
//! Default: 256 cases.
//!
//! Exit-gate run (release / dev fallback):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --test cross_parity \
//!       cross_chain_parity_holds
//! ```

use gsxdb_bridge::{Anchor, AnchorDispatcher, AnchorHash, ChainId, ParityResult, GENESIS_PARENT};
use gsxdb_bridge::anchor::{
    EcdsaSecp256k1Signer, VerifierConfig,
};
use gsxdb_state::Commitment;
use k256::ecdsa::SigningKey;
use proptest::prelude::*;
use rand::rngs::OsRng;

const NUM_CHAINS: u32 = 3;

fn dispatcher_with_chains() -> AnchorDispatcher {
    let mut d = AnchorDispatcher::new();
    for c in 1..=NUM_CHAINS {
        let mut key = [0u8; 32];
        key[0] = u8::try_from(c).unwrap_or(0);
        d.register(ChainId(c), key);
    }
    d
}

fn root_strategy() -> impl Strategy<Value = Commitment> {
    any::<[u8; 32]>().prop_map(Commitment)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S7 EXIT GATE.** For any sequence of dispatched `state_root`s,
    /// every height's parity check returns Agreed.
    #[test]
    fn cross_chain_parity_holds(
        roots in prop::collection::vec(root_strategy(), 1..16),
    ) {
        let mut d = dispatcher_with_chains();
        for (h, r) in roots.iter().enumerate() {
            let height = u64::try_from(h).unwrap();
            d.dispatch(height, *r).unwrap();
        }
        for (h, expected_root) in roots.iter().enumerate() {
            let height = u64::try_from(h).unwrap();
            match d.parity_check(height) {
                ParityResult::Agreed { state_root } => {
                    prop_assert_eq!(state_root, *expected_root);
                }
                other @ ParityResult::Disagreed { .. } => {
                    prop_assert!(false, "expected Agreed at height {height}, got {other:?}");
                }
            }
        }
    }

    /// Dispatched anchors appear on every registered chain at the
    /// expected height.
    #[test]
    fn dispatched_anchors_appear_on_all_chains(
        roots in prop::collection::vec(root_strategy(), 0..16),
    ) {
        let mut d = dispatcher_with_chains();
        for (h, r) in roots.iter().enumerate() {
            d.dispatch(u64::try_from(h).unwrap(), *r).unwrap();
        }
        for c in 1..=NUM_CHAINS {
            let log = d.log(ChainId(c)).unwrap();
            prop_assert_eq!(log.len(), roots.len());
        }
    }

    /// Tampering with one chain's log is detected by parity_check.
    #[test]
    fn parity_detects_tampering(
        roots in prop::collection::vec(root_strategy(), 1..8),
        tamper_chain in 1u32..=NUM_CHAINS,
        tamper_height_idx in 0usize..8,
    ) {
        let mut d = dispatcher_with_chains();
        for (h, r) in roots.iter().enumerate() {
            d.dispatch(u64::try_from(h).unwrap(), *r).unwrap();
        }
        let tamper_height = (tamper_height_idx % roots.len()) as u64;

        // Forge a different state root with a wrong MAC.
        let forged = Anchor {
            chain_id: ChainId(tamper_chain),
            height: tamper_height,
            state_root: Commitment([0xff; 32]),
            parent: GENESIS_PARENT,
            mac: [0; 32],
            auth_scheme: gsxdb_bridge::anchor::types::AuthScheme::Blake3Mac,
        };
        d.__log_mut_for_tests(ChainId(tamper_chain))
            .unwrap()
            .__tamper_for_tests(tamper_height, forged);

        match d.parity_check(tamper_height) {
            ParityResult::Disagreed { .. } => { /* expected */ }
            ParityResult::Agreed { .. } => prop_assert!(
                false,
                "tamper went undetected at height {tamper_height}"
            ),
        }
    }

    /// **S11.5 EXIT GATE** — mixed-scheme dispatcher. Two Blake3
    /// chains co-exist with one ECDSA chain; for every dispatched
    /// `state_root`, `parity_check` returns `Agreed` after both
    /// `dispatch` (Blake3) and `dispatch_with_signer` (ECDSA) write
    /// the same root at the same height.
    ///
    /// Exits at 10k cases under `PROPTEST_CASES=10000`; default 32
    /// because each case generates a fresh `SigningKey` plus N ECDSA
    /// signatures.
    #[test]
    fn mixed_blake3_and_ecdsa_parity_holds(
        roots in prop::collection::vec(root_strategy(), 1..8),
    ) {
        let signer = EcdsaSecp256k1Signer::new(SigningKey::random(&mut OsRng));
        let mut d = AnchorDispatcher::new();
        // Two Blake3 chains.
        d.register(ChainId(1), [1u8; 32]);
        d.register(ChainId(2), [2u8; 32]);
        // One ECDSA chain under the signer's own address.
        d.register_with_config(
            ChainId(3),
            VerifierConfig::EcdsaSecp256k1 { signer: signer.address() },
        );

        for (h, r) in roots.iter().enumerate() {
            let height = u64::try_from(h).unwrap();
            // Blake3 dispatch writes ChainId 1 and 2 (and fails on
            // ChainId 3 with SchemeRequiresSigner, which we ignore —
            // we'll write ChainId 3 explicitly via the signer).
            let _ = d.dispatch(height, *r);
            // ECDSA dispatch for ChainId 3.
            d.dispatch_with_signer(ChainId(3), height, *r, &signer)
                .expect("ECDSA dispatch must succeed under matched config");
        }

        for (h, expected_root) in roots.iter().enumerate() {
            let height = u64::try_from(h).unwrap();
            match d.parity_check(height) {
                ParityResult::Agreed { state_root } => {
                    prop_assert_eq!(state_root, *expected_root);
                }
                other @ ParityResult::Disagreed { .. } => {
                    prop_assert!(
                        false,
                        "expected Agreed across mixed-scheme dispatch at height {height}, got {other:?}"
                    );
                }
            }
        }
    }

    /// Anchor chain linkage holds: every anchor's parent matches the
    /// previous anchor's hash on the same chain.
    #[test]
    fn anchor_chain_is_linked(
        roots in prop::collection::vec(root_strategy(), 1..16),
    ) {
        let mut d = dispatcher_with_chains();
        for (h, r) in roots.iter().enumerate() {
            d.dispatch(u64::try_from(h).unwrap(), *r).unwrap();
        }
        for c in 1..=NUM_CHAINS {
            let log = d.log(ChainId(c)).unwrap();
            let mut expected = AnchorHash([0; 32]);
            for anchor in log {
                prop_assert_eq!(anchor.parent, expected);
                expected = anchor.hash();
            }
        }
    }
}
