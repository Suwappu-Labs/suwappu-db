//! Multi-chain anchor dispatcher + cross-chain parity check.

use super::credential::{
    verify_credential, AnchorAuthCredential, CredentialVerifyError, ExpectedVerifier,
};
use super::log::{AnchorLog, AppendError};
use super::types::{Anchor, AuthScheme, ChainId, GENESIS_PARENT};
use gsxdb_state::Commitment;
use std::collections::BTreeMap;

/// HARDENING rec 6.2 — hard-coded minimum quorum floor for the LTP
/// super-node attestation surface. Per the LTP paper §10.1 the
/// attestation quorum is 7-of-9; we set the absolute floor at 5/9 so
/// that any future reconfiguration that drops below honest-majority
/// (5/9 = first integer > 4) is rejected at the type level. KelpDAO
/// lost $292M when LayerZero allowed a single configurable DVN to
/// approve withdrawals.
/// Source: https://www.blockaid.io/blog/how-a-single-layerzero-dvn-compromise-drained-292m-from-kelpdao
pub const LTP_QUORUM_MIN_NUMERATOR: usize = 5;
/// LTP super-node committee size denominator (paper §3.2: seven of nine).
pub const LTP_QUORUM_DENOMINATOR: usize = 9;

/// Result of [`AnchorDispatcher::parity_check`] at one height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParityResult {
    /// Every registered chain anchored the same state root at this height.
    Agreed {
        /// The shared state root.
        state_root: Commitment,
    },
    /// At least two chains disagree, OR at least one chain has no
    /// anchor at this height. `divergent` is the per-chain (`chain_id`,
    /// `state_root`) for every chain that DID have an anchor — sorted
    /// by chain id.
    Disagreed {
        /// Chains that recorded an anchor at this height, with their
        /// state roots. Length is at least 1; if all `state_root`s
        /// were equal, [`ParityResult::Agreed`] would have been
        /// returned.
        divergent: Vec<(ChainId, Commitment)>,
        /// Chains with no anchor at this height.
        missing: Vec<ChainId>,
    },
}

/// Multi-chain dispatcher. Owns one [`AnchorLog`] per registered chain
/// plus the symmetric authenticator key for each.
#[derive(Debug, Clone)]
pub struct AnchorDispatcher {
    logs: BTreeMap<ChainId, AnchorLog>,
    keys: BTreeMap<ChainId, [u8; 32]>,
}

impl AnchorDispatcher {
    /// New dispatcher with no chains registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            logs: BTreeMap::new(),
            keys: BTreeMap::new(),
        }
    }

    /// Register a chain with its authenticator key. Replaces any
    /// existing registration. Existing log on `chain_id` is preserved.
    pub fn register(&mut self, chain_id: ChainId, key: [u8; 32]) {
        self.logs
            .entry(chain_id)
            .or_insert_with(|| AnchorLog::new(chain_id));
        self.keys.insert(chain_id, key);
    }

    /// Number of registered chains.
    #[must_use]
    pub fn len(&self) -> usize {
        self.logs.len()
    }

    /// `true` iff no chains registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.logs.is_empty()
    }

    /// Read-only access to a specific chain's log.
    #[must_use]
    pub fn log(&self, chain_id: ChainId) -> Option<&AnchorLog> {
        self.logs.get(&chain_id)
    }

    /// Mutable log access. Test-only — production code uses `dispatch`.
    #[doc(hidden)]
    pub fn __log_mut_for_tests(&mut self, chain_id: ChainId) -> Option<&mut AnchorLog> {
        self.logs.get_mut(&chain_id)
    }

    /// Dispatch a `(height, state_root)` to every registered chain.
    /// Each chain gets its own anchor (its own MAC under its own key,
    /// its own parent linkage to its previous anchor).
    ///
    /// Returns the dispatched anchors in chain-id order.
    ///
    /// # Errors
    ///
    /// Returns the first chain's [`AppendError`] on failure. Earlier
    /// chains' appends are **not** rolled back — the dispatcher is a
    /// best-effort multi-chain writer in phase-1; real-deploy adds a
    /// 2PC layer.
    ///
    /// # Panics
    ///
    /// Panics if `keys` and `logs` disagree on registered chains. This
    /// is an invariant maintained by [`Self::register`].
    pub fn dispatch(
        &mut self,
        height: u64,
        state_root: Commitment,
    ) -> Result<Vec<Anchor>, (ChainId, AppendError)> {
        let mut out = Vec::with_capacity(self.logs.len());
        let chain_ids: Vec<ChainId> = self.logs.keys().copied().collect();
        for chain_id in chain_ids {
            let key = *self
                .keys
                .get(&chain_id)
                .expect("registered keys match logs");
            let parent = self
                .logs
                .get(&chain_id)
                .and_then(AnchorLog::latest)
                .map_or(GENESIS_PARENT, Anchor::hash);
            let anchor = Anchor::new(chain_id, height, state_root, parent, &key);
            let log = self.logs.get_mut(&chain_id).expect("registered log exists");
            log.append(anchor.clone(), &key)
                .map_err(|e| (chain_id, e))?;
            out.push(anchor);
        }
        Ok(out)
    }

    /// Parity check at `height`: do all registered chains agree on
    /// the state root at this height?
    ///
    /// Returns [`ParityResult::Agreed`] iff every chain has an anchor
    /// at `height` AND all their `state_roots` are equal AND all their
    /// MACs verify under their respective keys. Otherwise
    /// [`ParityResult::Disagreed`] reports the divergent set + missing
    /// chains.
    ///
    /// # Panics
    ///
    /// Panics if `keys` and `logs` disagree on registered chains. See
    /// [`Self::dispatch`].
    #[must_use]
    pub fn parity_check(&self, height: u64) -> ParityResult {
        let mut roots: Vec<(ChainId, Commitment)> = Vec::new();
        let mut missing: Vec<ChainId> = Vec::new();
        let mut tampered: Vec<ChainId> = Vec::new();

        for (chain_id, log) in &self.logs {
            match log.at(height) {
                None => missing.push(*chain_id),
                Some(anchor) => {
                    let key = self.keys.get(chain_id).expect("registered keys match logs");
                    if !Self::verify_anchor_credential(anchor, key) {
                        // Tampered after-the-fact OR a non-Blake3Mac
                        // scheme without per-chain verifier config (S11
                        // adds the credential storage + verifier registry).
                        // Either way, treat as divergent and record so
                        // callers can see why.
                        tampered.push(*chain_id);
                    }
                    roots.push((*chain_id, anchor.state_root));
                }
            }
        }

        // If any chain is missing the height, parity fails.
        if !missing.is_empty() {
            return ParityResult::Disagreed {
                divergent: roots,
                missing,
            };
        }

        // If any anchor authenticator failed, parity fails.
        if !tampered.is_empty() {
            return ParityResult::Disagreed {
                divergent: roots,
                missing: tampered,
            };
        }

        // All chains have an anchor at this height with valid authenticators.
        // Agree iff every state_root is identical.
        if let Some((_, first)) = roots.first().copied() {
            if roots.iter().all(|(_, r)| *r == first) {
                ParityResult::Agreed { state_root: first }
            } else {
                ParityResult::Disagreed {
                    divergent: roots,
                    missing: Vec::new(),
                }
            }
        } else {
            // No chains registered at all.
            ParityResult::Disagreed {
                divergent: Vec::new(),
                missing: Vec::new(),
            }
        }
    }

    /// Route an anchor's authenticator through the unified
    /// [`verify_credential`] dispatch.
    ///
    /// Phase-1 (today): `Blake3Mac` is the only scheme stored in
    /// [`AnchorLog`], so the credential is the anchor's own `mac` field
    /// and the verifier config is the registered per-chain key.
    ///
    /// S11 (Track 1.2 Step E): `register` will accept a
    /// `VerifierConfig` enum carrying ECDSA / hybrid / Sp1 expected
    /// values, the log will store `(Anchor, AnchorAuthCredential)`
    /// pairs, and this helper will dispatch each scheme through
    /// `verify_credential` with the right `ExpectedVerifier`. Until
    /// then non-`Blake3Mac` anchors are rejected as
    /// `UnsupportedScheme` — preferable to silently treating them as
    /// authentic.
    fn verify_anchor_credential(anchor: &Anchor, key: &[u8; 32]) -> bool {
        match anchor.auth_scheme {
            AuthScheme::Blake3Mac => matches!(
                verify_credential(
                    anchor,
                    &AnchorAuthCredential::Blake3Mac,
                    &ExpectedVerifier::Blake3Mac { key },
                ),
                Ok(())
            ),
            AuthScheme::Sp1ZkProof | AuthScheme::EcdsaSecp256k1 | AuthScheme::MlDsa65Hybrid => {
                // No credential storage for non-MAC schemes yet.
                // S11 lands this surface; until then reject explicitly.
                let _ = CredentialVerifyError::UnsupportedScheme(anchor.auth_scheme);
                false
            }
        }
    }
}

impl Default for AnchorDispatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(byte: u8) -> Commitment {
        Commitment([byte; 32])
    }

    fn three_chain_dispatcher() -> AnchorDispatcher {
        let mut d = AnchorDispatcher::new();
        d.register(ChainId(1), [1; 32]);
        d.register(ChainId(2), [2; 32]);
        d.register(ChainId(3), [3; 32]);
        d
    }

    #[test]
    fn empty_dispatcher() {
        let d = AnchorDispatcher::new();
        assert!(d.is_empty());
        assert_eq!(d.len(), 0);
    }

    #[test]
    fn dispatch_to_all_registered_chains() {
        let mut d = three_chain_dispatcher();
        let anchors = d.dispatch(0, root(1)).unwrap();
        assert_eq!(anchors.len(), 3);
        for a in &anchors {
            assert_eq!(a.height, 0);
            assert_eq!(a.state_root, root(1));
        }
    }

    #[test]
    fn parity_agreed_after_clean_dispatch() {
        let mut d = three_chain_dispatcher();
        d.dispatch(0, root(1)).unwrap();
        d.dispatch(1, root(2)).unwrap();

        match d.parity_check(0) {
            ParityResult::Agreed { state_root } => assert_eq!(state_root, root(1)),
            other @ ParityResult::Disagreed { .. } => panic!("expected Agreed, got {other:?}"),
        }
        match d.parity_check(1) {
            ParityResult::Agreed { state_root } => assert_eq!(state_root, root(2)),
            other @ ParityResult::Disagreed { .. } => panic!("expected Agreed, got {other:?}"),
        }
    }

    #[test]
    fn parity_disagreed_when_height_missing() {
        let mut d = three_chain_dispatcher();
        d.dispatch(0, root(1)).unwrap();
        match d.parity_check(99) {
            ParityResult::Disagreed { missing, .. } => {
                assert_eq!(missing.len(), 3); // every chain missing
            }
            other @ ParityResult::Agreed { .. } => panic!("expected Disagreed, got {other:?}"),
        }
    }

    #[test]
    fn parity_disagreed_when_log_tampered() {
        let mut d = three_chain_dispatcher();
        d.dispatch(0, root(1)).unwrap();

        // Tamper with chain 2's anchor at height 0.
        let forged = Anchor {
            chain_id: ChainId(2),
            height: 0,
            state_root: root(99),
            parent: GENESIS_PARENT,
            mac: [0; 32], // invalid MAC
            auth_scheme: crate::anchor::types::AuthScheme::Blake3Mac,
        };
        d.__log_mut_for_tests(ChainId(2))
            .unwrap()
            .__tamper_for_tests(0, forged);

        match d.parity_check(0) {
            ParityResult::Disagreed { divergent, .. } => {
                // Chain 2's recorded root is now 99; chains 1 and 3 are
                // root(1).
                assert!(divergent
                    .iter()
                    .any(|(c, r)| *c == ChainId(2) && *r == root(99)));
            }
            other @ ParityResult::Agreed { .. } => panic!("expected Disagreed, got {other:?}"),
        }
    }

    #[test]
    fn parity_disagreed_when_anchor_scheme_swapped_without_credential() {
        // S11 wiring is not yet in place: a non-Blake3Mac anchor in the
        // log can't be verified through `verify_credential` (no per-chain
        // verifier config + no credential storage yet). Such anchors
        // surface as tampered, not as silently authentic.
        let mut d = three_chain_dispatcher();
        d.dispatch(0, root(1)).unwrap();

        let forged = Anchor {
            chain_id: ChainId(2),
            height: 0,
            state_root: root(1),
            parent: GENESIS_PARENT,
            mac: [0; 32],
            auth_scheme: crate::anchor::types::AuthScheme::EcdsaSecp256k1,
        };
        d.__log_mut_for_tests(ChainId(2))
            .unwrap()
            .__tamper_for_tests(0, forged);

        match d.parity_check(0) {
            ParityResult::Disagreed { .. } => {}
            other @ ParityResult::Agreed { .. } => {
                panic!("non-Blake3Mac scheme must not silently pass parity, got {other:?}")
            }
        }
    }

    #[test]
    fn anchor_hash_field_set_matches_solidity() {
        // Sanity check that Anchor::hash includes ONLY the fields
        // Solidity hashAnchor includes (chainId, height, stateRoot,
        // parent, mac) — NOT auth_scheme. Changing auth_scheme on an
        // otherwise-identical anchor MUST NOT change the hash.
        let base = Anchor {
            chain_id: ChainId(7),
            height: 42,
            state_root: Commitment([0xAB; 32]),
            parent: GENESIS_PARENT,
            mac: [0x11; 32],
            auth_scheme: crate::anchor::types::AuthScheme::Blake3Mac,
        };
        let twin_ecdsa = Anchor {
            auth_scheme: crate::anchor::types::AuthScheme::EcdsaSecp256k1,
            ..base.clone()
        };
        let twin_hybrid = Anchor {
            auth_scheme: crate::anchor::types::AuthScheme::MlDsa65Hybrid,
            ..base.clone()
        };
        let twin_sp1 = Anchor {
            auth_scheme: crate::anchor::types::AuthScheme::Sp1ZkProof,
            ..base.clone()
        };
        assert_eq!(base.hash(), twin_ecdsa.hash());
        assert_eq!(base.hash(), twin_hybrid.hash());
        assert_eq!(base.hash(), twin_sp1.hash());
    }

    #[test]
    fn dispatch_chains_anchors_via_parent_linkage() {
        let mut d = three_chain_dispatcher();
        d.dispatch(0, root(1)).unwrap();
        d.dispatch(1, root(2)).unwrap();
        d.dispatch(2, root(3)).unwrap();

        let log = d.log(ChainId(1)).unwrap();
        assert_eq!(log.len(), 3);
        let a0 = log.at(0).unwrap();
        let a1 = log.at(1).unwrap();
        let a2 = log.at(2).unwrap();
        assert_eq!(a1.parent, a0.hash());
        assert_eq!(a2.parent, a1.hash());
    }

    #[test]
    fn registering_existing_chain_replaces_key_only() {
        let mut d = AnchorDispatcher::new();
        d.register(ChainId(1), [1; 32]);
        d.dispatch(0, root(1)).unwrap();

        d.register(ChainId(1), [9; 32]);
        // Log preserved; old anchor still there.
        assert_eq!(d.log(ChainId(1)).unwrap().len(), 1);
        // But subsequent dispatches use the new key — the old anchor
        // won't verify under it.
        let old_anchor = d.log(ChainId(1)).unwrap().at(0).unwrap().clone();
        assert!(!old_anchor.verify_auth(&[9; 32]));
    }
}
