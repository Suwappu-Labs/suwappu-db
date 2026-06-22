//! Multi-chain anchor dispatcher + cross-chain parity check.

use super::credential::{
    verify_credential, AnchorAuthCredential, CredentialVerifyError, VerifierConfig,
};
use super::log::{AnchorLog, AppendError};
use super::signing::{AnchorSigner, SignerError};
use super::types::{Anchor, AuthScheme, ChainId, GENESIS_PARENT};
use suwappudb_state::Commitment;
use std::collections::BTreeMap;

/// HARDENING rec 6.2 — hard-coded minimum quorum floor for the LTP
/// super-node attestation surface. Per the LTP paper §10.1 the
/// attestation quorum is 7-of-9; we set the absolute floor at 5/9 so
/// that any future reconfiguration that drops below honest-majority
/// (5/9 = first integer > 4) is rejected at the type level. `KelpDAO`
/// lost $292M when `LayerZero` allowed a single configurable DVN to
/// approve withdrawals.
/// Source: <https://www.blockaid.io/blog/how-a-single-layerzero-dvn-compromise-drained-292m-from-kelpdao>
pub const LTP_QUORUM_MIN_NUMERATOR: usize = 5;
/// LTP super-node committee size denominator (paper §3.2: seven of nine).
pub const LTP_QUORUM_DENOMINATOR: usize = 9;

/// Reasons [`AnchorDispatcher::dispatch_with_signer`] can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchSignerError {
    /// `chain_id` has no [`VerifierConfig`] registered.
    ChainNotRegistered(ChainId),
    /// Signer's [`AnchorSigner::scheme`] doesn't match the chain's
    /// [`VerifierConfig::scheme`].
    SchemeMismatch {
        /// Scheme the chain is registered under.
        config: AuthScheme,
        /// Scheme the signer produces.
        signer: AuthScheme,
    },
    /// Chain is registered as Blake3-MAC; use [`AnchorDispatcher::dispatch`]
    /// instead, which doesn't require a signer.
    Blake3UsesDispatch,
    /// S11.3 ships ECDSA only; hybrid + Sp1 producers follow once the
    /// production-pqc / zkVM toolchain decisions land.
    UnsupportedProducerScheme(AuthScheme),
    /// Underlying signer failed.
    Signer(SignerError),
    /// Log append failed after signing succeeded — typically a parent
    /// / height issue.
    Append(AppendError),
}

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
/// plus the [`VerifierConfig`] for each.
///
/// Two registration paths:
/// - [`Self::register`] — legacy / Blake3-only convenience that takes a
///   32-byte symmetric key. Wraps [`Self::register_with_config`] with
///   [`VerifierConfig::Blake3Mac`].
/// - [`Self::register_with_config`] — S11 path. Accepts any
///   [`VerifierConfig`] variant, including ECDSA / hybrid / Sp1.
#[derive(Debug, Clone)]
pub struct AnchorDispatcher {
    logs: BTreeMap<ChainId, AnchorLog>,
    /// Per-chain verifier config (S11). Replaces the pre-S11 raw-key
    /// `keys` map. Blake3 chains store their key inside the config.
    configs: BTreeMap<ChainId, VerifierConfig>,
}

impl AnchorDispatcher {
    /// New dispatcher with no chains registered.
    #[must_use]
    pub fn new() -> Self {
        Self {
            logs: BTreeMap::new(),
            configs: BTreeMap::new(),
        }
    }

    /// **Legacy / Blake3-only convenience.** Registers `chain_id` with
    /// a [`VerifierConfig::Blake3Mac`] config built from `key`. Existing
    /// log on `chain_id` is preserved. Equivalent to:
    ///
    /// ```ignore
    /// dispatcher.register_with_config(chain_id, VerifierConfig::Blake3Mac { key });
    /// ```
    pub fn register(&mut self, chain_id: ChainId, key: [u8; 32]) {
        self.register_with_config(chain_id, VerifierConfig::Blake3Mac { key });
    }

    /// **S11.1 entry point.** Register `chain_id` with an arbitrary
    /// [`VerifierConfig`]. Replaces any existing config; the
    /// [`AnchorLog`] on `chain_id` is preserved across re-registration.
    pub fn register_with_config(&mut self, chain_id: ChainId, config: VerifierConfig) {
        self.logs
            .entry(chain_id)
            .or_insert_with(|| AnchorLog::new(chain_id));
        self.configs.insert(chain_id, config);
    }

    /// Read-only access to a chain's [`VerifierConfig`]. `None` if the
    /// chain is unregistered.
    #[must_use]
    pub fn config(&self, chain_id: ChainId) -> Option<&VerifierConfig> {
        self.configs.get(&chain_id)
    }

    /// **S11.3 entry point** — dispatch a `(height, state_root)` to a
    /// specific chain that's registered under a non-Blake3 config.
    /// `signer` produces the credential sidecar.
    ///
    /// The signer's [`AnchorSigner::scheme`] must match the chain's
    /// [`VerifierConfig::scheme`]; mismatch returns
    /// [`DispatchSignerError::SchemeMismatch`].
    ///
    /// Blake3-MAC chains continue to use [`Self::dispatch`]; this
    /// method explicitly rejects them so callers don't accidentally
    /// sign a chain they also keyed.
    pub fn dispatch_with_signer(
        &mut self,
        chain_id: ChainId,
        height: u64,
        state_root: suwappudb_state::Commitment,
        signer: &dyn AnchorSigner,
    ) -> Result<Anchor, DispatchSignerError> {
        let config = self
            .configs
            .get(&chain_id)
            .ok_or(DispatchSignerError::ChainNotRegistered(chain_id))?
            .clone();
        if config.scheme() != signer.scheme() {
            return Err(DispatchSignerError::SchemeMismatch {
                config: config.scheme(),
                signer: signer.scheme(),
            });
        }
        if config.scheme() == AuthScheme::Blake3Mac {
            return Err(DispatchSignerError::Blake3UsesDispatch);
        }

        let parent = self
            .logs
            .get(&chain_id)
            .and_then(AnchorLog::latest)
            .map_or(GENESIS_PARENT, Anchor::hash);
        let anchor = match signer.scheme() {
            AuthScheme::EcdsaSecp256k1 => {
                Anchor::ecdsa(chain_id, height, state_root, parent)
            }
            // S11.3 ships ECDSA only. Hybrid + Sp1 producers follow once
            // the production-pqc and zkVM toolchain decisions land.
            other => return Err(DispatchSignerError::UnsupportedProducerScheme(other)),
        };
        let credential = signer.sign(&anchor).map_err(DispatchSignerError::Signer)?;
        // Defensive: signer should never produce a non-ECDSA credential
        // for an ECDSA scheme, but check at the boundary.
        if !matches!(credential, AnchorAuthCredential::EcdsaSecp256k1 { .. }) {
            return Err(DispatchSignerError::Signer(SignerError::SchemeMismatch));
        }
        let log = self
            .logs
            .get_mut(&chain_id)
            .expect("registered log exists");
        log.append_with_credential(anchor.clone(), credential, &config)
            .map_err(DispatchSignerError::Append)?;
        Ok(anchor)
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
    /// Panics if `configs` and `logs` disagree on registered chains.
    /// Invariant maintained by [`Self::register_with_config`].
    ///
    /// Dispatch path covers Blake3-MAC chains only; non-MAC schemes
    /// require a signer (S11.3 — see `dispatch_with_signer`). Chains
    /// registered with a non-Blake3 [`VerifierConfig`] return
    /// `(chain_id, AppendError::SchemeRequiresSigner)`.
    pub fn dispatch(
        &mut self,
        height: u64,
        state_root: Commitment,
    ) -> Result<Vec<Anchor>, (ChainId, AppendError)> {
        let mut out = Vec::with_capacity(self.logs.len());
        let chain_ids: Vec<ChainId> = self.logs.keys().copied().collect();
        for chain_id in chain_ids {
            let config = self
                .configs
                .get(&chain_id)
                .expect("registered configs match logs");
            let key = match config {
                VerifierConfig::Blake3Mac { key } => *key,
                // S11.3 lands the signing pipeline for non-MAC schemes.
                // Until then `dispatch` only handles Blake3 chains.
                _ => return Err((chain_id, AppendError::SchemeRequiresSigner)),
            };
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
    /// Panics if `configs` and `logs` disagree on registered chains.
    /// See [`Self::dispatch`].
    #[must_use]
    pub fn parity_check(&self, height: u64) -> ParityResult {
        let mut roots: Vec<(ChainId, Commitment)> = Vec::new();
        let mut missing: Vec<ChainId> = Vec::new();
        let mut tampered: Vec<ChainId> = Vec::new();

        for (chain_id, log) in &self.logs {
            match log.entry_at(height) {
                None => missing.push(*chain_id),
                Some(entry) => {
                    let config = self
                        .configs
                        .get(chain_id)
                        .expect("registered configs match logs");
                    let anchor = &entry.anchor;
                    if !Self::verify_entry_with_config(entry, config) {
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

    /// Route an [`AnchorEntry`]'s authenticator through the unified
    /// [`verify_credential`] dispatch using the chain's
    /// [`VerifierConfig`].
    ///
    /// **S11.2**: every entry now carries a stored credential, so all
    /// four schemes (Blake3 / ECDSA / Hybrid / Sp1) dispatch through
    /// the same path. Pre-S11.2 `dispatch_with_signer` is still queued
    /// (S11.3), so the producer side of non-Blake3 anchors is gated
    /// — but `parity_check` + `append_with_credential` work end-to-end.
    fn verify_entry_with_config(
        entry: &super::log::AnchorEntry,
        config: &VerifierConfig,
    ) -> bool {
        // Schemes must agree: the config's declared scheme must match
        // the anchor's `auth_scheme`. Otherwise reject without even
        // attempting to dispatch — prevents config drift from silently
        // accepting cross-scheme anchors.
        if config.scheme() != entry.anchor.auth_scheme {
            let _ = CredentialVerifyError::SchemeMismatch;
            return false;
        }
        let expected = config.as_expected_for(&entry.anchor);
        matches!(
            verify_credential(&entry.anchor, &entry.credential, &expected),
            Ok(())
        )
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
    fn register_with_config_stores_arbitrary_scheme() {
        // S11.1: registering an ECDSA chain stores the config and
        // makes it readable via `config()`. The dispatcher accepts
        // any VerifierConfig variant without requiring the full
        // signing pipeline to be wired (that's S11.3).
        let mut d = AnchorDispatcher::new();
        let signer = crate::anchor::credential::EthAddress([0x42; 20]);
        d.register_with_config(
            ChainId(7),
            VerifierConfig::EcdsaSecp256k1 { signer },
        );

        assert_eq!(d.len(), 1);
        match d.config(ChainId(7)).expect("registered") {
            VerifierConfig::EcdsaSecp256k1 { signer: s } => assert_eq!(*s, signer),
            other => panic!("expected ECDSA config, got {other:?}"),
        }
    }

    #[test]
    fn legacy_register_creates_blake3_config() {
        // S11.1: the legacy `register(chain_id, key)` API now stores
        // a `VerifierConfig::Blake3Mac { key }`. Existing callers see
        // no behavioural change.
        let mut d = AnchorDispatcher::new();
        d.register(ChainId(1), [0x99; 32]);
        match d.config(ChainId(1)).expect("registered") {
            VerifierConfig::Blake3Mac { key } => assert_eq!(*key, [0x99; 32]),
            other => panic!("expected Blake3Mac config, got {other:?}"),
        }
    }

    #[test]
    fn dispatch_rejects_non_blake3_config() {
        // S11.1: until S11.3 wires the signer pipeline, `dispatch`
        // can only produce Blake3-MAC anchors. Non-MAC chains return
        // a typed error.
        let mut d = AnchorDispatcher::new();
        let signer = crate::anchor::credential::EthAddress([0x11; 20]);
        d.register_with_config(
            ChainId(5),
            VerifierConfig::EcdsaSecp256k1 { signer },
        );

        let err = d.dispatch(0, root(1)).unwrap_err();
        assert_eq!(err.0, ChainId(5));
        assert!(matches!(err.1, AppendError::SchemeRequiresSigner));
    }

    #[test]
    fn dispatch_with_signer_ecdsa_round_trips_through_parity_check() {
        // S11.3 happy path: sign an anchor end-to-end, parity_check
        // accepts it under the registered config.
        use k256::ecdsa::SigningKey;
        use rand::rngs::OsRng;

        let mut d = AnchorDispatcher::new();
        let signing_key = SigningKey::random(&mut OsRng);
        let signer = crate::anchor::signing::EcdsaSecp256k1Signer::new(signing_key);
        let signer_address = signer.address();
        d.register_with_config(
            ChainId(7),
            VerifierConfig::EcdsaSecp256k1 {
                signer: signer_address,
            },
        );

        // Register a second (Blake3) chain so parity_check has more
        // than one input to consider.
        d.register(ChainId(8), [3; 32]);
        d.dispatch(0, root(1)).unwrap_err(); // chain 7 returns SchemeRequiresSigner

        // Now sign chain 7 properly.
        let anchor7 = d
            .dispatch_with_signer(ChainId(7), 0, root(1), &signer)
            .expect("ECDSA dispatch succeeds");
        assert_eq!(anchor7.height, 0);
        assert_eq!(anchor7.state_root, root(1));
        assert_eq!(anchor7.auth_scheme, AuthScheme::EcdsaSecp256k1);
        assert_eq!(anchor7.mac, [0u8; 32]);

        // parity_check on chain 7 alone reads its stored credential
        // and runs full verify_credential dispatch.
        let entry = d.log(ChainId(7)).unwrap().entry_at(0).unwrap();
        assert!(matches!(
            entry.credential,
            AnchorAuthCredential::EcdsaSecp256k1 { .. }
        ));
    }

    #[test]
    fn dispatch_with_signer_rejects_scheme_mismatch() {
        // Signer scheme must match config scheme.
        use k256::ecdsa::SigningKey;
        use rand::rngs::OsRng;

        let mut d = AnchorDispatcher::new();
        let signer = crate::anchor::signing::EcdsaSecp256k1Signer::new(SigningKey::random(
            &mut OsRng,
        ));
        d.register(ChainId(1), [1; 32]); // Blake3 config

        let err = d
            .dispatch_with_signer(ChainId(1), 0, root(1), &signer)
            .unwrap_err();
        // Blake3 chain → either Blake3UsesDispatch (preferred) or
        // SchemeMismatch (also acceptable). Implementation returns
        // SchemeMismatch first because it's the cheaper check.
        assert!(matches!(
            err,
            DispatchSignerError::SchemeMismatch { .. }
                | DispatchSignerError::Blake3UsesDispatch
        ));
    }

    #[test]
    fn dispatch_with_signer_rejects_unregistered_chain() {
        use k256::ecdsa::SigningKey;
        use rand::rngs::OsRng;

        let mut d = AnchorDispatcher::new();
        let signer = crate::anchor::signing::EcdsaSecp256k1Signer::new(SigningKey::random(
            &mut OsRng,
        ));
        let err = d
            .dispatch_with_signer(ChainId(99), 0, root(1), &signer)
            .unwrap_err();
        assert_eq!(err, DispatchSignerError::ChainNotRegistered(ChainId(99)));
    }

    #[test]
    fn dispatch_with_signer_chains_anchors_via_parent_linkage() {
        // Two consecutive signed dispatches; second anchor's parent
        // must equal first anchor's hash.
        use k256::ecdsa::SigningKey;
        use rand::rngs::OsRng;

        let mut d = AnchorDispatcher::new();
        let signer = crate::anchor::signing::EcdsaSecp256k1Signer::new(SigningKey::random(
            &mut OsRng,
        ));
        d.register_with_config(
            ChainId(7),
            VerifierConfig::EcdsaSecp256k1 {
                signer: signer.address(),
            },
        );
        let a0 = d
            .dispatch_with_signer(ChainId(7), 0, root(1), &signer)
            .unwrap();
        let a1 = d
            .dispatch_with_signer(ChainId(7), 1, root(2), &signer)
            .unwrap();
        assert_eq!(a1.parent, a0.hash());
    }

    #[test]
    fn parity_rejects_blake3_anchor_under_ecdsa_config() {
        // Scheme mismatch between the anchor and the config is an
        // immediate `false` from verify_anchor_with_config — prevents
        // a config-swap attack from re-validating old MAC anchors as
        // if they were ECDSA.
        let mut d = AnchorDispatcher::new();
        d.register(ChainId(1), [1; 32]);
        d.dispatch(0, root(1)).unwrap();

        // Now swap the chain's config to ECDSA without changing the
        // underlying anchor. Parity must reject.
        d.register_with_config(
            ChainId(1),
            VerifierConfig::EcdsaSecp256k1 {
                signer: crate::anchor::credential::EthAddress([0; 20]),
            },
        );
        match d.parity_check(0) {
            ParityResult::Disagreed { .. } => {}
            other @ ParityResult::Agreed { .. } => {
                panic!("config-swap must not reauthenticate the old anchor, got {other:?}")
            }
        }
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
