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

/// Per-entry payload size for a balance entry in `encoded_state`.
/// 20 bytes address + 16 bytes canonical balance (u128 LE) = 36.
pub const SNAPSHOT_ENTRY_BYTES: usize = 36;

/// V1 magic — balance-only snapshots: `magic || N×(addr||bal_le)`.
/// Still read for back-compat; never written by [`StateSnapshot::from_state`].
const SNAPSHOT_MAGIC_V1: &[u8; 8] = b"SUWAPPU\x01";

/// V2 magic — captures the balance column AND the bytes column:
/// `magic || bal_count:u32le || N×(addr||bal_le) || byt_count:u32le ||
/// M×(addr||len:u32le||data)`. All entries ascending-by-address.
/// This is what `from_state` writes once the bytes column exists, so a
/// snapshot can no longer silently drop bytes-state.
const SNAPSHOT_MAGIC_V2: &[u8; 8] = b"SUWAPPU\x02";

/// Decoded body of a snapshot: balances + the bytes column.
struct DecodedBody {
    balances: Vec<(Address, u128)>,
    bytes: Vec<(Address, Vec<u8>)>,
}

/// Parse a V2 `encoded_state` body (everything after the 8-byte magic).
/// Bounds-checked; returns a descriptive error on any truncation.
fn parse_v2_body(enc: &[u8]) -> Result<DecodedBody, String> {
    let body = &enc[SNAPSHOT_MAGIC_V2.len()..];
    let mut cur = 0usize;
    let take_u32 = |body: &[u8], cur: &mut usize| -> Result<u32, String> {
        if *cur + 4 > body.len() {
            return Err("snapshot v2: truncated length prefix".to_string());
        }
        let n = u32::from_le_bytes(body[*cur..*cur + 4].try_into().unwrap());
        *cur += 4;
        Ok(n)
    };

    let bal_count = take_u32(body, &mut cur)? as usize;
    let mut balances = Vec::with_capacity(bal_count);
    for _ in 0..bal_count {
        if cur + SNAPSHOT_ENTRY_BYTES > body.len() {
            return Err("snapshot v2: truncated balance entry".to_string());
        }
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&body[cur..cur + 20]);
        let mut bal = [0u8; 16];
        bal.copy_from_slice(&body[cur + 20..cur + 36]);
        balances.push((Address(addr), u128::from_le_bytes(bal)));
        cur += SNAPSHOT_ENTRY_BYTES;
    }

    let byt_count = take_u32(body, &mut cur)? as usize;
    let mut bytes = Vec::with_capacity(byt_count);
    for _ in 0..byt_count {
        if cur + 24 > body.len() {
            return Err("snapshot v2: truncated bytes header".to_string());
        }
        let mut addr = [0u8; 20];
        addr.copy_from_slice(&body[cur..cur + 20]);
        let len = u32::from_le_bytes(body[cur + 20..cur + 24].try_into().unwrap()) as usize;
        cur += 24;
        if cur + len > body.len() {
            return Err("snapshot v2: truncated bytes payload".to_string());
        }
        bytes.push((Address(addr), body[cur..cur + len].to_vec()));
        cur += len;
    }

    if cur != body.len() {
        return Err(format!(
            "snapshot v2: {} trailing bytes after parse",
            body.len() - cur
        ));
    }
    Ok(DecodedBody { balances, bytes })
}

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
    /// Writes the **V2** format (magic `SUWAPPU\x02`): the balance column
    /// followed by the bytes column, each section length-prefixed and sorted
    /// by address. Capturing the bytes column is what stops a snapshot from
    /// silently dropping protocol-registry state.
    /// **Determinism (S12.5 fix)**: entries are sorted by address before
    /// encoding so the byte-stream is independent of the store's iteration
    /// order — `InMemoryBalanceStore`'s balance map is a `HashMap` and would
    /// otherwise yield bytewise non-idempotent
    /// `from_state ∘ restore_into_state ∘ from_state` round-trips.
    ///
    /// The `state_root` field is left as `Commitment([0; 32])` because
    /// the state-tree root is computed separately; use
    /// [`Self::with_state_root`] to fill it in.
    #[must_use]
    pub fn from_state(state: &State, height: u64, anchor_hash: Option<[u8; 32]>) -> Self {
        // V2 format: both the balance column and the bytes column. Both
        // sections are sorted by address so the byte-stream is independent
        // of store iteration order (idempotent round-trips).
        let mut balances = state.entries();
        balances.sort_by_key(|a| a.0 .0);
        let mut bytes = state.bytes_entries();
        bytes.sort_by_key(|a| a.0 .0);

        let mut encoded = Vec::with_capacity(
            SNAPSHOT_MAGIC_V2.len() + 8 + balances.len() * SNAPSHOT_ENTRY_BYTES,
        );
        encoded.extend_from_slice(SNAPSHOT_MAGIC_V2);

        let bal_count = u32::try_from(balances.len()).expect("balance count exceeds u32");
        encoded.extend_from_slice(&bal_count.to_le_bytes());
        for (addr, slot) in &balances {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(&slot.canonical().to_le_bytes());
        }

        let byt_count = u32::try_from(bytes.len()).expect("bytes count exceeds u32");
        encoded.extend_from_slice(&byt_count.to_le_bytes());
        for (addr, data) in &bytes {
            let len = u32::try_from(data.len()).expect("bytes value exceeds u32");
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(&len.to_le_bytes());
            encoded.extend_from_slice(data);
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
        // Cheap structural check — caught here so callers don't have to
        // pre-validate before `restore_into_state`.
        snapshot.validate_encoding()?;
        Ok(snapshot)
    }

    /// Validate the `encoded_state` header + structure for the detected
    /// snapshot version (V1 balance-only or V2 balance+bytes).
    fn validate_encoding(&self) -> Result<(), String> {
        let enc = &self.encoded_state;
        if enc.len() < 8 {
            return Err("snapshot too short for magic".to_string());
        }
        match &enc[..8] {
            m if m == SNAPSHOT_MAGIC_V1 => {
                let body_len = enc.len() - SNAPSHOT_MAGIC_V1.len();
                if body_len % SNAPSHOT_ENTRY_BYTES != 0 {
                    return Err(format!(
                        "snapshot v1 body length {body_len} not a multiple of {SNAPSHOT_ENTRY_BYTES}"
                    ));
                }
                Ok(())
            }
            m if m == SNAPSHOT_MAGIC_V2 => parse_v2_body(enc).map(|_| ()),
            other => Err(format!("snapshot magic mismatch (got {other:?})")),
        }
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
    pub fn restore_into_state(
        &self,
        state: &mut State,
        token: &BridgeToken,
    ) -> Result<usize, String> {
        let enc = &self.encoded_state;
        if enc.len() < 8 {
            return Err("snapshot too short for magic".to_string());
        }
        match &enc[..8] {
            // V1: balance-only, fixed 36-byte entries.
            m if m == SNAPSHOT_MAGIC_V1 => {
                let body = &enc[SNAPSHOT_MAGIC_V1.len()..];
                if body.len() % SNAPSHOT_ENTRY_BYTES != 0 {
                    return Err(format!(
                        "snapshot v1 body length {} not a multiple of {SNAPSHOT_ENTRY_BYTES}",
                        body.len()
                    ));
                }
                let mut applied = 0usize;
                for chunk in body.chunks_exact(SNAPSHOT_ENTRY_BYTES) {
                    let mut addr_bytes = [0u8; 20];
                    addr_bytes.copy_from_slice(&chunk[..20]);
                    let mut bal_bytes = [0u8; 16];
                    bal_bytes.copy_from_slice(&chunk[20..]);
                    state.apply(
                        token,
                        &StateChange::SetBalance {
                            addr: Address(addr_bytes),
                            to: Balance(u128::from_le_bytes(bal_bytes)),
                        },
                    );
                    applied += 1;
                }
                Ok(applied)
            }
            // V2: balances + bytes column. Returns total changes applied.
            m if m == SNAPSHOT_MAGIC_V2 => {
                let decoded = parse_v2_body(enc)?;
                let mut applied = 0usize;
                for (addr, bal) in decoded.balances {
                    state.apply(
                        token,
                        &StateChange::SetBalance {
                            addr,
                            to: Balance(bal),
                        },
                    );
                    applied += 1;
                }
                for (addr, data) in decoded.bytes {
                    state.apply(token, &StateChange::SetBytes { addr, bytes: data });
                    applied += 1;
                }
                Ok(applied)
            }
            _ => Err("snapshot magic mismatch".to_string()),
        }
    }

    /// **S12.2** — number of entries the encoded body claims.
    /// Returns `None` if the body is malformed.
    #[must_use]
    pub fn entry_count(&self) -> Option<usize> {
        let enc = &self.encoded_state;
        if enc.len() < 8 {
            return None;
        }
        match &enc[..8] {
            m if m == SNAPSHOT_MAGIC_V1 => {
                let body_len = enc.len() - SNAPSHOT_MAGIC_V1.len();
                if body_len % SNAPSHOT_ENTRY_BYTES != 0 {
                    return None;
                }
                Some(body_len / SNAPSHOT_ENTRY_BYTES)
            }
            m if m == SNAPSHOT_MAGIC_V2 => parse_v2_body(enc).ok().map(|d| d.balances.len()),
            _ => None,
        }
    }

    /// Number of bytes-column entries the snapshot carries. `Some(0)` for a
    /// V1 (balance-only) snapshot; `None` if the body is malformed.
    #[must_use]
    pub fn bytes_entry_count(&self) -> Option<usize> {
        let enc = &self.encoded_state;
        if enc.len() < 8 {
            return None;
        }
        match &enc[..8] {
            m if m == SNAPSHOT_MAGIC_V1 => Some(0),
            m if m == SNAPSHOT_MAGIC_V2 => parse_v2_body(enc).ok().map(|d| d.bytes.len()),
            _ => None,
        }
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
        // V2 body is self-describing; a truncation surfaces as a parse error.
        assert!(err.contains("truncated") || err.contains("v2"), "got: {err}");
    }

    #[test]
    fn snapshot_v2_round_trips_balances_and_bytes() {
        // The whole point of v2: bytes-column entries survive the
        // from_state → restore round-trip (v1 would silently drop them).
        let mut original = State::default();
        let token = BridgeToken::__for_bridge_only();
        for i in 1u8..=4 {
            original.apply(
                &token,
                &StateChange::SetBalance {
                    addr: Address([i; 20]),
                    to: Balance(u128::from(i) * 100),
                },
            );
        }
        original.apply(
            &token,
            &StateChange::SetBytes {
                addr: Address([0xAA; 20]),
                bytes: vec![1, 2, 3, 4, 5],
            },
        );
        original.apply(
            &token,
            &StateChange::SetBytes {
                addr: Address([0xBB; 20]),
                bytes: vec![9],
            },
        );

        let snap = StateSnapshot::from_state(&original, 7, None);
        assert_eq!(snap.entry_count(), Some(4));
        assert_eq!(snap.bytes_entry_count(), Some(2));

        let mut restored = State::default();
        snap.restore_into_state(&mut restored, &token).expect("restore");
        for i in 1u8..=4 {
            assert_eq!(
                restored.balance_of(&Address([i; 20])),
                Balance(u128::from(i) * 100)
            );
        }
        assert_eq!(restored.bytes_of(&Address([0xAA; 20])), Some(vec![1, 2, 3, 4, 5]));
        assert_eq!(restored.bytes_of(&Address([0xBB; 20])), Some(vec![9]));
    }

    #[test]
    fn snapshot_from_state_writes_v2_magic() {
        let snap = StateSnapshot::from_state(&State::default(), 1, None);
        assert_eq!(&snap.encoded_state[..8], SNAPSHOT_MAGIC_V2);
    }

    #[test]
    fn snapshot_v1_back_compat_restores_balances() {
        // A legacy v1 (balance-only) snapshot must still load, with the
        // bytes column simply empty.
        let mut enc = Vec::new();
        enc.extend_from_slice(SNAPSHOT_MAGIC_V1);
        enc.extend_from_slice(&Address([5; 20]).0);
        enc.extend_from_slice(&777u128.to_le_bytes());
        let snap = StateSnapshot::new(3, Commitment([0; 32]), enc, None);
        assert_eq!(snap.entry_count(), Some(1));
        assert_eq!(snap.bytes_entry_count(), Some(0));

        let token = BridgeToken::__for_bridge_only();
        let mut restored = State::default();
        let applied = snap.restore_into_state(&mut restored, &token).expect("restore v1");
        assert_eq!(applied, 1);
        assert_eq!(restored.balance_of(&Address([5; 20])), Balance(777));
        assert_eq!(restored.bytes_of(&Address([5; 20])), None);
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
