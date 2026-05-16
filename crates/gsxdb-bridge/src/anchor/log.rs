//! Append-only per-chain anchor log.

use super::credential::{
    verify_credential, AnchorAuthCredential, CredentialVerifyError, VerifierConfig,
};
use super::types::{Anchor, AnchorHash, AuthScheme, AuthVerifyError, ChainId, GENESIS_PARENT};

/// Reasons an [`AnchorLog::append`] can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppendError {
    /// Anchor's `chain_id` doesn't match the log's `chain_id`.
    ChainMismatch {
        /// Log's chain id.
        log: ChainId,
        /// Anchor's chain id.
        anchor: ChainId,
    },
    /// Anchor's parent doesn't match the previous anchor's hash (or
    /// `GENESIS_PARENT` for the first append).
    ParentMismatch {
        /// What the log expected.
        expected: AnchorHash,
        /// What the anchor claimed.
        got: AnchorHash,
    },
    /// Anchor's height isn't `prev.height + 1` (or 0 for genesis).
    HeightGap {
        /// Expected height.
        expected: u64,
        /// Got height.
        got: u64,
    },
    /// Anchor authenticator doesn't verify under the provided key.
    BadAuth,
    /// Anchor uses an auth scheme that has no verifier in this phase.
    UnsupportedAuthScheme(AuthScheme),
    /// **S11.1**: dispatcher was asked to produce an anchor under a
    /// non-Blake3 [`super::credential::VerifierConfig`] but no signer
    /// was supplied. The dispatcher's `dispatch` path only emits
    /// Blake3-MAC anchors; non-MAC chains require the S11.3
    /// `dispatch_with_signer` entry point.
    SchemeRequiresSigner,
    /// **S11.2**: credential supplied to [`AnchorLog::append_with_credential`]
    /// failed the unified `verify_credential` dispatch. Carries the
    /// specific reason so callers can distinguish a malformed signature
    /// from a scheme mismatch from an unsupported scheme.
    CredentialInvalid(CredentialVerifyError),
}

/// One entry in an [`AnchorLog`] — the anchor plus its authentication
/// sidecar. For `Blake3Mac` chains the credential is
/// [`AnchorAuthCredential::Blake3Mac`] (no extra bytes; the MAC lives
/// inside the anchor). For non-MAC schemes the credential carries the
/// signature bytes that wouldn't fit in [`Anchor::mac`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorEntry {
    /// The anchor record.
    pub anchor: Anchor,
    /// The authentication sidecar.
    pub credential: AnchorAuthCredential,
}

/// Append-only per-chain log of anchors + their auth credentials.
///
/// Each entry is an [`AnchorEntry`] pairing the anchor with its
/// auth sidecar. Blake3-MAC chains' entries carry
/// [`AnchorAuthCredential::Blake3Mac`] (no extra bytes — the MAC is
/// in the anchor); non-MAC chains carry the real signature bytes.
#[derive(Debug, Clone)]
pub struct AnchorLog {
    chain_id: ChainId,
    entries: Vec<AnchorEntry>,
}

impl AnchorLog {
    /// New empty log for `chain_id`.
    #[must_use]
    pub fn new(chain_id: ChainId) -> Self {
        Self {
            chain_id,
            entries: Vec::new(),
        }
    }

    /// **Blake3-only convenience**, retained from the pre-S11.2
    /// surface. Wraps [`Self::append_with_credential`] with a
    /// [`AnchorAuthCredential::Blake3Mac`] sidecar and a
    /// [`VerifierConfig::Blake3Mac`] config.
    ///
    /// # Errors
    ///
    /// Returns the specific [`AppendError`] for whichever check fails;
    /// no append happens on error.
    pub fn append(&mut self, anchor: Anchor, key: &[u8; 32]) -> Result<(), AppendError> {
        // Pre-S11.2 callers verified directly via
        // `anchor.verify_auth_result(key)` and got
        // `AppendError::{BadAuth,UnsupportedAuthScheme}` rather than the
        // unified `CredentialInvalid` variant. Keep that surface stable
        // by checking the legacy path first, then delegating storage to
        // `append_with_credential` once it passes.
        if anchor.chain_id == self.chain_id {
            match anchor.verify_auth_result(key) {
                Ok(()) => {}
                Err(AuthVerifyError::InvalidAuthenticator) => {
                    // Drop into the credential path so parent / height
                    // checks also run; the credential check below will
                    // fail with `CredentialInvalid` which we translate
                    // back to `BadAuth` for the legacy caller surface.
                }
                Err(AuthVerifyError::UnsupportedScheme(scheme)) => {
                    return Err(AppendError::UnsupportedAuthScheme(scheme));
                }
            }
        }
        let config = VerifierConfig::Blake3Mac { key: *key };
        let result = self.append_with_credential(anchor, AnchorAuthCredential::Blake3Mac, &config);
        match result {
            Err(AppendError::CredentialInvalid(CredentialVerifyError::Blake3MacInvalid)) => {
                Err(AppendError::BadAuth)
            }
            other => other,
        }
    }

    /// **S11.2 entry point.** Append `anchor` with `credential`,
    /// validating chain id, parent linkage, height monotonicity, and
    /// running the unified [`verify_credential`] dispatch against
    /// `config`.
    ///
    /// # Errors
    ///
    /// - [`AppendError::ChainMismatch`] — anchor's `chain_id` differs
    ///   from the log's.
    /// - [`AppendError::ParentMismatch`] / [`AppendError::HeightGap`]
    ///   — anchor doesn't append cleanly.
    /// - [`AppendError::CredentialInvalid`] — `verify_credential`
    ///   rejected. Carries the specific failure reason.
    pub fn append_with_credential(
        &mut self,
        anchor: Anchor,
        credential: AnchorAuthCredential,
        config: &VerifierConfig,
    ) -> Result<(), AppendError> {
        if anchor.chain_id != self.chain_id {
            return Err(AppendError::ChainMismatch {
                log: self.chain_id,
                anchor: anchor.chain_id,
            });
        }
        let (expected_parent, expected_height) = match self.entries.last() {
            Some(prev) => (prev.anchor.hash(), prev.anchor.height + 1),
            None => (GENESIS_PARENT, 0),
        };
        if anchor.parent != expected_parent {
            return Err(AppendError::ParentMismatch {
                expected: expected_parent,
                got: anchor.parent,
            });
        }
        if anchor.height != expected_height {
            return Err(AppendError::HeightGap {
                expected: expected_height,
                got: anchor.height,
            });
        }
        let expected = config.as_expected_for(&anchor);
        if let Err(e) = verify_credential(&anchor, &credential, &expected) {
            return Err(AppendError::CredentialInvalid(e));
        }
        self.entries.push(AnchorEntry { anchor, credential });
        Ok(())
    }

    /// Chain this log records.
    #[must_use]
    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    /// Anchor at logical height, if recorded. O(1) since heights are
    /// dense and start at 0.
    #[must_use]
    pub fn at(&self, height: u64) -> Option<&Anchor> {
        Some(&self.entry_at(height)?.anchor)
    }

    /// **S11.2** — full entry (anchor + credential) at logical height.
    #[must_use]
    pub fn entry_at(&self, height: u64) -> Option<&AnchorEntry> {
        let idx = usize::try_from(height).ok()?;
        self.entries.get(idx)
    }

    /// **S11.2** — credential at logical height. Used by the
    /// dispatcher's parity_check to dispatch through
    /// `verify_credential`.
    #[must_use]
    pub fn credential_at(&self, height: u64) -> Option<&AnchorAuthCredential> {
        Some(&self.entry_at(height)?.credential)
    }

    /// Latest anchor on this chain, or `None` if empty.
    #[must_use]
    pub fn latest(&self) -> Option<&Anchor> {
        Some(&self.entries.last()?.anchor)
    }

    /// **S11.2** — latest entry (anchor + credential), or `None` if empty.
    #[must_use]
    pub fn latest_entry(&self) -> Option<&AnchorEntry> {
        self.entries.last()
    }

    /// Number of anchors.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `true` iff no anchors recorded.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate every anchor in order.
    pub fn iter(&self) -> impl Iterator<Item = &Anchor> {
        self.entries.iter().map(|e| &e.anchor)
    }

    /// **S11.2** — iterate full entries (anchor + credential) in order.
    pub fn iter_entries(&self) -> std::slice::Iter<'_, AnchorEntry> {
        self.entries.iter()
    }
}

impl<'a> IntoIterator for &'a AnchorLog {
    type Item = &'a Anchor;
    type IntoIter = std::iter::Map<std::slice::Iter<'a, AnchorEntry>, fn(&AnchorEntry) -> &Anchor>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter().map(|e| &e.anchor)
    }
}

impl AnchorLog {
    /// Tamper helper for property tests — replace the anchor at
    /// `height` with a forged anchor (keeping its existing credential).
    /// **Tests only.** Real code never calls this; mutating an
    /// append-only log is a contract violation.
    #[doc(hidden)]
    pub fn __tamper_for_tests(&mut self, height: u64, replacement: Anchor) {
        if let Ok(idx) = usize::try_from(height) {
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.anchor = replacement;
            }
        }
    }

    /// **S11.2 tamper helper** — replace the credential at `height`
    /// for property tests that need to drive credential-mismatch
    /// rejections through the verify path.
    #[doc(hidden)]
    pub fn __tamper_credential_for_tests(
        &mut self,
        height: u64,
        replacement: AnchorAuthCredential,
    ) {
        if let Ok(idx) = usize::try_from(height) {
            if let Some(entry) = self.entries.get_mut(idx) {
                entry.credential = replacement;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::credential;
    use gsxdb_state::Commitment;

    fn root(byte: u8) -> Commitment {
        Commitment([byte; 32])
    }
    const KEY: [u8; 32] = [9; 32];

    #[test]
    fn empty_log_basics() {
        let log = AnchorLog::new(ChainId(1));
        assert_eq!(log.len(), 0);
        assert!(log.is_empty());
        assert!(log.latest().is_none());
        assert!(log.at(0).is_none());
    }

    #[test]
    fn append_genesis_anchor() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        log.append(a.clone(), &KEY).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log.latest(), Some(&a));
        assert_eq!(log.at(0), Some(&a));
    }

    #[test]
    fn append_chain_links_correctly() {
        let mut log = AnchorLog::new(ChainId(1));
        let a0 = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        log.append(a0.clone(), &KEY).unwrap();

        let a1 = Anchor::new(ChainId(1), 1, root(2), a0.hash(), &KEY);
        log.append(a1.clone(), &KEY).unwrap();

        assert_eq!(log.len(), 2);
        assert_eq!(log.at(1), Some(&a1));
        assert_eq!(log.latest(), Some(&a1));
    }

    #[test]
    fn append_rejects_chain_mismatch() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(2), 0, root(1), GENESIS_PARENT, &KEY);
        let err = log.append(a, &KEY).unwrap_err();
        assert_eq!(
            err,
            AppendError::ChainMismatch {
                log: ChainId(1),
                anchor: ChainId(2)
            }
        );
        assert!(log.is_empty());
    }

    #[test]
    fn append_rejects_wrong_parent() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), AnchorHash([1; 32]), &KEY);
        let err = log.append(a, &KEY).unwrap_err();
        assert!(matches!(err, AppendError::ParentMismatch { .. }));
    }

    #[test]
    fn append_rejects_height_gap() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 5, root(1), GENESIS_PARENT, &KEY);
        let err = log.append(a, &KEY).unwrap_err();
        assert_eq!(
            err,
            AppendError::HeightGap {
                expected: 0,
                got: 5
            }
        );
    }

    #[test]
    fn append_rejects_bad_auth() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        let wrong_key = [0; 32];
        let err = log.append(a, &wrong_key).unwrap_err();
        assert_eq!(err, AppendError::BadAuth);
    }

    #[test]
    fn append_with_credential_blake3_round_trip() {
        // S11.2: explicit append_with_credential path mirrors the
        // legacy append for Blake3. credential_at returns the stored
        // sidecar.
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        let config = VerifierConfig::Blake3Mac { key: KEY };
        log.append_with_credential(a.clone(), AnchorAuthCredential::Blake3Mac, &config)
            .unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log.at(0), Some(&a));
        assert!(matches!(
            log.credential_at(0),
            Some(AnchorAuthCredential::Blake3Mac)
        ));
    }

    #[test]
    fn append_with_credential_rejects_scheme_mismatch() {
        // Anchor is Blake3 but the config is ECDSA — verify_credential
        // returns SchemeMismatch and the log refuses the append.
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        let config = VerifierConfig::EcdsaSecp256k1 {
            signer: credential::EthAddress([0; 20]),
        };
        let err = log
            .append_with_credential(a, AnchorAuthCredential::Blake3Mac, &config)
            .unwrap_err();
        assert!(matches!(
            err,
            AppendError::CredentialInvalid(CredentialVerifyError::SchemeMismatch)
        ));
        assert!(log.is_empty());
    }

    #[test]
    fn iter_entries_yields_credentials() {
        // S11.2: full-entry iterator gives both halves; the legacy
        // `iter` still yields just the anchors.
        let mut log = AnchorLog::new(ChainId(1));
        let a0 = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        log.append(a0.clone(), &KEY).unwrap();
        let a1 = Anchor::new(ChainId(1), 1, root(2), a0.hash(), &KEY);
        log.append(a1.clone(), &KEY).unwrap();

        let entries: Vec<_> = log.iter_entries().collect();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].anchor, a0);
        assert_eq!(entries[1].anchor, a1);
        for e in &entries {
            assert!(matches!(e.credential, AnchorAuthCredential::Blake3Mac));
        }

        // Legacy iter shape preserved.
        let anchors: Vec<_> = log.iter().collect();
        assert_eq!(anchors.len(), 2);
        assert_eq!(anchors[0], &a0);
    }

    #[test]
    fn tamper_credential_helper_drives_credential_mismatch() {
        // Drop a Blake3 credential and replace it with a phantom
        // ECDSA one. verify_credential will refuse on the next
        // parity_check because the anchor's auth_scheme is Blake3 but
        // the credential variant is ECDSA — SchemeMismatch.
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        log.append(a, &KEY).unwrap();
        log.__tamper_credential_for_tests(
            0,
            AnchorAuthCredential::EcdsaSecp256k1 {
                signature: [0u8; credential::ECDSA_SIG_LEN],
            },
        );
        let entry = log.entry_at(0).unwrap();
        assert!(matches!(
            entry.credential,
            AnchorAuthCredential::EcdsaSecp256k1 { .. }
        ));
    }

    #[test]
    fn append_rejects_skipped_height_after_genesis() {
        let mut log = AnchorLog::new(ChainId(1));
        let a0 = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        log.append(a0.clone(), &KEY).unwrap();
        let a2 = Anchor::new(ChainId(1), 2, root(2), a0.hash(), &KEY);
        let err = log.append(a2, &KEY).unwrap_err();
        assert_eq!(
            err,
            AppendError::HeightGap {
                expected: 1,
                got: 2
            }
        );
    }
}
