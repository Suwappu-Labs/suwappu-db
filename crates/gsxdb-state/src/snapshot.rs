//! State snapshots for fast recovery and cross-validation.
//!
//! Snapshots capture the full state (balances, nonces, storage) at a block height
//! and can be exported/imported for:
//! - Fast startup from recent snapshot + delta replay
//! - State export for cross-chain validation
//! - Rollback to known-good state

use crate::Commitment;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

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
    pub fn size_bytes(&self) -> usize {
        // height (8) + state_root (32) + timestamp (8) + anchor_hash (32 or 0) + encoded_state
        8 + 32 + 8 + self.anchor_hash.map_or(0, |_| 32) + self.encoded_state.len()
    }

    /// Check if snapshot is valid:
    /// - height > 0
    /// - timestamp is recent (not in future, not too old)
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
    /// Returns true if anchor_hash matches the snapshot's anchor_hash field.
    pub fn verify_anchor(&self, anchor_hash: &[u8; 32]) -> bool {
        self.anchor_hash.as_ref() == Some(anchor_hash)
    }

    /// JSON representation for metadata storage.
    pub fn to_metadata_json(&self) -> serde_json::Value {
        serde_json::json!({
            "height": self.height,
            "state_root": format!("0x{}", hex::encode(&self.state_root.0)),
            "timestamp": self.timestamp,
            "size_bytes": self.size_bytes(),
            "anchor_hash": self.anchor_hash.map(|h| format!("0x{}", hex::encode(&h))),
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
    pub fn new(snapshot_dir: String, snapshot_interval: u64) -> Self {
        Self {
            snapshot_dir,
            snapshot_interval,
            max_age_secs: 7 * 24 * 60 * 60, // 7 days
            max_snapshots: 3,
        }
    }

    /// Check if a snapshot should be taken at this height.
    pub fn should_snapshot(&self, height: u64) -> bool {
        height > 0 && height % self.snapshot_interval == 0
    }

    /// Generate snapshot filename for the given height and root.
    pub fn snapshot_filename(&self, height: u64, state_root: &Commitment) -> String {
        let root_hex = hex::encode(&state_root.0[..8]);
        format!("snapshot-{:06}-{}.json", height, root_hex)
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
                    snapshot.anchor_hash.map(|h| hex::encode(&h))
                ));
            }
        }

        Ok(())
    }
}

impl Default for SnapshotManager {
    fn default() -> Self {
        Self::new("/tmp/gsxdb-snapshots".to_string(), 1000)
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
