//! Append-only per-chain anchor log.

use super::types::{Anchor, AnchorHash, ChainId, GENESIS_PARENT};

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
    /// Anchor's MAC doesn't verify under the provided key.
    BadMac,
}

/// Append-only per-chain log of anchors.
#[derive(Debug, Clone)]
pub struct AnchorLog {
    chain_id: ChainId,
    entries: Vec<Anchor>,
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

    /// Append `anchor`. Validates chain id, parent linkage, height
    /// monotonicity, and MAC under `key`.
    ///
    /// # Errors
    ///
    /// Returns the specific [`AppendError`] for whichever check fails;
    /// no append happens on error.
    pub fn append(&mut self, anchor: Anchor, key: &[u8; 32]) -> Result<(), AppendError> {
        if anchor.chain_id != self.chain_id {
            return Err(AppendError::ChainMismatch {
                log: self.chain_id,
                anchor: anchor.chain_id,
            });
        }
        let (expected_parent, expected_height) = match self.entries.last() {
            Some(prev) => (prev.hash(), prev.height + 1),
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
        if !anchor.verify_mac(key) {
            return Err(AppendError::BadMac);
        }
        self.entries.push(anchor);
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
        let idx = usize::try_from(height).ok()?;
        self.entries.get(idx)
    }

    /// Latest anchor on this chain, or `None` if empty.
    #[must_use]
    pub fn latest(&self) -> Option<&Anchor> {
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
    pub fn iter(&self) -> std::slice::Iter<'_, Anchor> {
        self.entries.iter()
    }
}

impl<'a> IntoIterator for &'a AnchorLog {
    type Item = &'a Anchor;
    type IntoIter = std::slice::Iter<'a, Anchor>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl AnchorLog {
    /// Tamper helper for property tests — replace the entry at
    /// `height` with a forged anchor. **Tests only.** Real code never
    /// calls this; mutating an append-only log is a contract violation.
    #[doc(hidden)]
    pub fn __tamper_for_tests(&mut self, height: u64, replacement: Anchor) {
        if let Ok(idx) = usize::try_from(height) {
            if idx < self.entries.len() {
                self.entries[idx] = replacement;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn append_rejects_bad_mac() {
        let mut log = AnchorLog::new(ChainId(1));
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &KEY);
        let wrong_key = [0; 32];
        let err = log.append(a, &wrong_key).unwrap_err();
        assert_eq!(err, AppendError::BadMac);
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
