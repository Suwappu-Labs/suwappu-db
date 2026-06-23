//! State snapshots for fast recovery and cross-validation.
//!
//! Snapshots capture the full state (balances, nonces, storage) at a block height
//! and can be exported/imported for:
//! - Fast startup from recent snapshot + delta replay
//! - State export for cross-chain validation
//! - Rollback to known-good state

use crate::{Address, Balance, BalanceSlot, BridgeToken, Commitment, State, StateChange};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-entry payload size for [`StateSnapshot::encoded_state`].
/// 20 bytes address + 16 bytes canonical balance (u128 LE) = 36.
pub const SNAPSHOT_ENTRY_BYTES: usize = 36;

/// Magic header prepended to `encoded_state` so a corrupted or
/// mistyped file fails loudly instead of silently restoring zero
/// balances.
const SNAPSHOT_MAGIC: &[u8; 8] = b"SUWAPPU\x01";

/// A snapshot of the entire state at a specific block height.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateSnapshot {
    /// Block height at snapshot time.
    pub height: u64,
    /// State root commitment (tree root).
    pub state_root: Commitment,
    /// Serialized state blob (redb binary format or similar).
    /// In production, this would be compressed.
    pub encoded_state: Vec<u8>,
    /// Unix timestamp of snapshot creation.
    pub timestamp: u64,
    /// Optional anchor hash for cross-chain verification.
    /// If set, can verify snapshot matches on-chain anchor at this height.
    pub anchor_hash: Option<[u8; 32]>,
}

impl StateSnapshot {
    /// Create a new snapshot.
    #[must_use]
    pub fn new(
        height: u64,
        state_root: Commitment,
        encoded_state: Vec<u8>,
        anchor_hash: Option<[u8; 32]>,
    ) -> Self {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        Self {
            height,
            state_root,
            encoded_state,
            timestamp,
            anchor_hash,
        }
    }

    /// Serialized size in bytes.
    #[must_use]
    pub fn size_bytes(&self) -> usize {
        // height (8) + state_root (32) + timestamp (8) + anchor_hash (32 or 0) + encoded_state
        8 + 32 + 8 + self.anchor_hash.map_or(0, |_| 32) + self.encoded_state.len()
    }

    /// Check if snapshot is valid:
    /// - height > 0
    /// - timestamp is recent (not in future, not too old)
    ///
    /// **B4 audit note**: this freshness gate reads `SystemTime::now`
    /// at validation time. Local clock skew can reject otherwise-valid
    /// snapshots; operators with drift should widen `max_age_secs`.
    /// Not a soundness issue — snapshot equality / restore correctness
    /// do not depend on the timestamp, only this acceptance window.
    #[must_use]
    pub fn is_valid(&self, max_age_secs: u64) -> bool {
        if self.height == 0 {
            return false; // Genesis snapshot unlikely
        }

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // Not in the future
        if self.timestamp > now {
            return false;
        }

        // Not older than max_age_secs
        if now - self.timestamp > max_age_secs {
            return false;
        }

        // Sanity check: encoded state not empty
        !self.encoded_state.is_empty()
    }

    /// Verify snapshot matches an anchor hash.
    /// Returns true if `anchor_hash` matches the snapshot's `anchor_hash` field.
    #[must_use]
    pub fn verify_anchor(&self, anchor_hash: &[u8; 32]) -> bool {
        self.anchor_hash.as_ref() == Some(anchor_hash)
    }

    /// **S12.2** — capture a snapshot from a live [`State`].
    ///
    /// Encodes every `(addr, slot.canonical())` as a 36-byte tuple in
    /// `encoded_state` prefixed by the [`SNAPSHOT_MAGIC`] header.
    /// **Determinism (S12.5 fix)**: entries are sorted by address
    /// before encoding so the byte-stream is independent of the
    /// store's iteration order — `InMemoryBalanceStore` uses a
    /// `HashMap` and would otherwise yield bytewise non-idempotent
    /// `from_state ∘ restore_into_state ∘ from_state` round-trips.
    ///
    /// The `state_root` field is left as `Commitment([0; 32])` because
    /// the state-tree root is computed separately; use
    /// [`Self::with_state_root`] to fill it in.
    #[must_use]
    pub fn from_state(state: &State, height: u64, anchor_hash: Option<[u8; 32]>) -> Self {
        let mut entries = state.entries();
        entries.sort_by_key(|a| a.0 .0);
        let mut encoded =
            Vec::with_capacity(SNAPSHOT_MAGIC.len() + entries.len() * SNAPSHOT_ENTRY_BYTES);
        encoded.extend_from_slice(SNAPSHOT_MAGIC);
        for (addr, slot) in &entries {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(&slot.canonical().to_le_bytes());
        }
        Self::new(height, Commitment([0; 32]), encoded, anchor_hash)
    }

    /// Builder-style setter for the post-`from_state` state root.
    #[must_use]
    pub fn with_state_root(mut self, root: Commitment) -> Self {
        self.state_root = root;
        self
    }

    /// **S12.2** — write this snapshot to `path` as pretty-printed
    /// JSON. The `encoded_state` blob is hex-encoded inside the JSON
    /// envelope so files are line-diffable.
    pub fn write_to_file<P: AsRef<Path>>(&self, path: P) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| format!("serialize: {e}"))?;
        std::fs::write(path, bytes).map_err(|e| format!("write: {e}"))
    }

    /// **S12.2** — read a snapshot from `path`.
    pub fn read_from_file<P: AsRef<Path>>(path: P) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("read: {e}"))?;
        let snapshot: Self =
            serde_json::from_slice(&bytes).map_err(|e| format!("deserialize: {e}"))?;
        // Cheap header check — caught here so callers don't have to
        // pre-validate before `restore_into_state`.
        if snapshot.encoded_state.len() < SNAPSHOT_MAGIC.len()
            || &snapshot.encoded_state[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC
        {
            return Err(format!(
                "snapshot magic mismatch (expected {SNAPSHOT_MAGIC:?})"
            ));
        }
        let body_len = snapshot.encoded_state.len() - SNAPSHOT_MAGIC.len();
        if body_len % SNAPSHOT_ENTRY_BYTES != 0 {
            return Err(format!(
                "snapshot body length {body_len} not a multiple of {SNAPSHOT_ENTRY_BYTES}"
            ));
        }
        Ok(snapshot)
    }

    /// **S12.2** — restore the encoded entries into `state` via the
    /// bridge token. Existing entries are left in place unless they're
    /// overwritten by the snapshot's addresses; for a hard restore,
    /// reset the state externally first.
    ///
    /// Returns the number of entries applied.
    ///
    /// # Errors
    ///
    /// Returns a string describing the failure if `encoded_state` is
    /// malformed (missing magic, non-multiple length, etc.).
    pub fn restore_into_state(&self, state: &mut State, token: &BridgeToken) -> Result<usize, String> {
        if self.encoded_state.len() < SNAPSHOT_MAGIC.len()
            || &self.encoded_state[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC
        {
            return Err("snapshot magic mismatch".to_string());
        }
        let body = &self.encoded_state[SNAPSHOT_MAGIC.len()..];
        if body.len() % SNAPSHOT_ENTRY_BYTES != 0 {
            return Err(format!(
                "snapshot body length {} not a multiple of {SNAPSHOT_ENTRY_BYTES}",
                body.len()
            ));
        }
        let mut applied = 0usize;
        for chunk in body.chunks_exact(SNAPSHOT_ENTRY_BYTES) {
            let mut addr_bytes = [0u8; 20];
            addr_bytes.copy_from_slice(&chunk[..20]);
            let mut bal_bytes = [0u8; 16];
            bal_bytes.copy_from_slice(&chunk[20..]);
            let value = u128::from_le_bytes(bal_bytes);
            state.apply(
                token,
                &StateChange::SetBalance {
                    addr: Address(addr_bytes),
                    to: Balance(value),
                },
            );
            applied += 1;
        }
        Ok(applied)
    }

    /// **S12.2** — number of entries the encoded body claims.
    /// Returns `None` if the body is malformed.
    #[must_use]
    pub fn entry_count(&self) -> Option<usize> {
        if self.encoded_state.len() < SNAPSHOT_MAGIC.len()
            || &self.encoded_state[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC
        {
            return None;
        }
        let body_len = self.encoded_state.len() - SNAPSHOT_MAGIC.len();
        if body_len % SNAPSHOT_ENTRY_BYTES != 0 {
            return None;
        }
        Some(body_len / SNAPSHOT_ENTRY_BYTES)
    }

    /// Unused stub kept until `BalanceSlot` is reachable directly.
    #[doc(hidden)]
    #[must_use]
    pub fn __load_balance_slot_for_tests(byte: u8) -> BalanceSlot {
        BalanceSlot::new(u128::from(byte))
    }

    /// JSON representation for metadata storage.
    #[must_use]
    pub fn to_metadata_json(&self) -> serde_json::Value {
        serde_json::json!({
            "height": self.height,
            "state_root": format!("0x{}", hex::encode(self.state_root.0)),
            "timestamp": self.timestamp,
            "size_bytes": self.size_bytes(),
            "anchor_hash": self.anchor_hash.map(|h| format!("0x{}", hex::encode(h))),
        })
    }
}

/// Snapshot manager for periodic export and import.
#[derive(Debug, Clone)]
pub struct SnapshotManager {
    /// Directory where snapshots are stored.
    pub snapshot_dir: String,
    /// Interval in blocks between snapshots.
    pub snapshot_interval: u64,
    /// Maximum age of a snapshot in seconds (default 7 days).
    pub max_age_secs: u64,
    /// Maximum number of snapshots to keep (older ones are pruned).
    pub max_snapshots: usize,
}

impl SnapshotManager {
    /// Create a new snapshot manager.
    #[must_use]
    pub fn new(snapshot_dir: String, snapshot_interval: u64) -> Self {
        Self {
            snapshot_dir,
            snapshot_interval,
            max_age_secs: 7 * 24 * 60 * 60, // 7 days
            max_snapshots: 3,
        }
    }

    /// Check if a snapshot should be taken at this height.
    #[must_use]
    pub fn should_snapshot(&self, height: u64) -> bool {
        height > 0 && height % self.snapshot_interval == 0
    }

    /// Generate snapshot filename for the given height and root.
    #[must_use]
    pub fn snapshot_filename(&self, height: u64, state_root: &Commitment) -> String {
        let root_hex = hex::encode(&state_root.0[..8]);
        format!("snapshot-{height:06}-{root_hex}.json")
    }

    /// Validate snapshot: check metadata and anchor consistency.
    pub fn validate_snapshot(
        &self,
        snapshot: &StateSnapshot,
        expected_anchor_hash: Option<&[u8; 32]>,
    ) -> Result<(), String> {
        if !snapshot.is_valid(self.max_age_secs) {
            return Err("Snapshot validation failed: invalid timestamp or empty state".to_string());
        }

        if let Some(expected_anchor) = expected_anchor_hash {
            if !snapshot.verify_anchor(expected_anchor) {
                return Err(format!(
                    "Snapshot anchor mismatch: expected {:?}, got {:?}",
                    hex::encode(expected_anchor),
                    snapshot.anchor_hash.map(hex::encode)
                ));
            }
        }

        Ok(())
    }
}

impl Default for SnapshotManager {
    fn default() -> Self {
        Self::new("/tmp/suwappudb-snapshots".to_string(), 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(byte: u8) -> Commitment {
        Commitment([byte; 32])
    }

    #[test]
    fn snapshot_new_has_timestamp() {
        let snapshot = StateSnapshot::new(100, root(7), vec![1, 2, 3, 4, 5], Some([1u8; 32]));

        assert_eq!(snapshot.height, 100);
        assert_eq!(snapshot.state_root, root(7));
        assert!(snapshot.timestamp > 0);
    }

    #[test]
    fn snapshot_size_includes_all_fields() {
        let snapshot = StateSnapshot::new(42, root(1), vec![0u8; 1000], Some([2u8; 32]));

        let size = snapshot.size_bytes();
        // height (8) + root (32) + timestamp (8) + anchor (32) + data (1000) = 1080
        assert_eq!(size, 1080);
    }

    #[test]
    fn snapshot_is_valid_checks_height() {
        // Genesis should be invalid
        let genesis_snapshot = StateSnapshot::new(0, root(1), vec![1], None);
        assert!(!genesis_snapshot.is_valid(u64::MAX));

        // Normal snapshot should be valid
        let normal_snapshot = StateSnapshot::new(1, root(1), vec![1], None);
        assert!(normal_snapshot.is_valid(u64::MAX));
    }

    #[test]
    fn snapshot_is_valid_checks_timestamp_age() {
        let old_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 10 * 24 * 60 * 60; // 10 days ago

        let mut snapshot = StateSnapshot::new(1, root(1), vec![1], None);
        snapshot.timestamp = old_timestamp;

        // Too old (max 7 days)
        assert!(!snapshot.is_valid(7 * 24 * 60 * 60));
    }

    #[test]
    fn snapshot_verify_anchor_matches() {
        let anchor = [42u8; 32];
        let snapshot = StateSnapshot::new(10, root(3), vec![1, 2, 3], Some(anchor));

        assert!(snapshot.verify_anchor(&anchor));
        assert!(!snapshot.verify_anchor(&[99u8; 32]));
    }

    #[test]
    fn snapshot_to_metadata_json() {
        let snapshot = StateSnapshot::new(50, root(7), vec![1; 500], Some([9u8; 32]));

        let json = snapshot.to_metadata_json();
        assert_eq!(json["height"], 50);
        assert!(json["state_root"].is_string());
        assert!(json["timestamp"].is_number());
        assert!(json["anchor_hash"].is_string());
    }

    #[test]
    fn snapshot_from_state_then_restore_round_trips() {
        // S12.2 core: build a state, snapshot it, restore into a
        // fresh state, assert balance-by-balance equality.
        let mut original = State::default();
        let token = BridgeToken::__for_bridge_only();
        for i in 1u8..=8 {
            original.apply(
                &token,
                &StateChange::SetBalance {
                    addr: Address([i; 20]),
                    to: Balance(u128::from(i) * 100),
                },
            );
        }

        let snapshot = StateSnapshot::from_state(&original, 10, Some([0xAB; 32]));
        assert_eq!(snapshot.entry_count(), Some(8));

        let mut restored = State::default();
        let applied = snapshot
            .restore_into_state(&mut restored, &token)
            .expect("restore");
        assert_eq!(applied, 8);
        for i in 1u8..=8 {
            assert_eq!(
                restored.balance_of(&Address([i; 20])),
                Balance(u128::from(i) * 100),
                "mismatch at addr {i}"
            );
        }
    }

    #[test]
    fn snapshot_file_io_round_trips() {
        // S12.2: write + read round-trip through a tempfile.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([7; 20]),
                to: Balance(424_242),
            },
        );
        let snapshot = StateSnapshot::from_state(&state, 1, None)
            .with_state_root(Commitment([0xCC; 32]));

        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("snap.json");
        snapshot.write_to_file(&path).expect("write");
        let loaded = StateSnapshot::read_from_file(&path).expect("read");
        assert_eq!(loaded.height, snapshot.height);
        assert_eq!(loaded.state_root, snapshot.state_root);
        assert_eq!(loaded.encoded_state, snapshot.encoded_state);
        assert_eq!(loaded.entry_count(), Some(1));
    }

    #[test]
    fn snapshot_read_rejects_missing_magic() {
        let snap = StateSnapshot::new(1, root(1), b"not-magic-prefix-followed-by-noise".to_vec(), None);
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("bad.json");
        snap.write_to_file(&path).expect("write");
        let err = StateSnapshot::read_from_file(&path).unwrap_err();
        assert!(err.contains("magic"), "got: {err}");
    }

    #[test]
    fn snapshot_read_rejects_truncated_body() {
        // Header present but body not a multiple of SNAPSHOT_ENTRY_BYTES.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([1; 20]),
                to: Balance(1),
            },
        );
        let mut snap = StateSnapshot::from_state(&state, 1, None);
        snap.encoded_state.pop(); // truncate the last byte
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("truncated.json");
        snap.write_to_file(&path).expect("write");
        let err = StateSnapshot::read_from_file(&path).unwrap_err();
        assert!(err.contains("multiple"), "got: {err}");
    }

    #[test]
    fn snapshot_manager_should_snapshot() {
        let manager = SnapshotManager::new("/tmp".to_string(), 1000);

        assert!(!manager.should_snapshot(0));
        assert!(!manager.should_snapshot(999));
        assert!(manager.should_snapshot(1000));
        assert!(manager.should_snapshot(2000));
        assert!(!manager.should_snapshot(1001));
    }

    #[test]
    fn snapshot_manager_filename() {
        let manager = SnapshotManager::new("/tmp".to_string(), 1000);
        let filename = manager.snapshot_filename(100, &root(7));

        assert!(filename.contains("snapshot-"));
        assert!(filename.contains("000100")); // height 100 zero-padded
    }

    #[test]
    fn snapshot_manager_validate_success() {
        let manager = SnapshotManager::new("/tmp".to_string(), 1000);
        let anchor = [5u8; 32];
        let snapshot = StateSnapshot::new(10, root(1), vec![1; 100], Some(anchor));

        assert!(manager.validate_snapshot(&snapshot, Some(&anchor)).is_ok());
    }

    #[test]
    fn snapshot_manager_validate_fails_on_anchor_mismatch() {
        let manager = SnapshotManager::new("/tmp".to_string(), 1000);
        let snapshot = StateSnapshot::new(10, root(1), vec![1; 100], Some([5u8; 32]));

        let result = manager.validate_snapshot(&snapshot, Some(&[99u8; 32]));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("anchor mismatch"));
    }
}
