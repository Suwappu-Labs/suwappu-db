//! State snapshots for fast recovery and cross-validation.
//!
//! Snapshots capture the full state (balances, nonces, storage) at a block height
//! and can be exported/imported for:
//! - Fast startup from recent snapshot + delta replay
//! - State export for cross-chain validation
//! - Rollback to known-good state

use crate::{Address, Balance, BalanceSlot, BridgeToken, Commitment, State, StateChange};
use serde::{Deserialize, Serialize};
use sha3::Digest;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Per-entry payload size for [`StateSnapshot::encoded_state`].
/// 20 bytes address + 16 bytes canonical balance (u128 LE) = 36.
pub const SNAPSHOT_ENTRY_BYTES: usize = 36;

/// Magic header prepended to `encoded_state`. `\x03` is the sectioned
/// format carrying balances + EVM contract state (code / storage /
/// account-code) + the reserved-address `bytes_state` registry, so a
/// snapshot captures the full `state_root` rather than balances alone. A
/// mismatched magic fails loudly instead of silently restoring partial state.
const SNAPSHOT_MAGIC: &[u8; 8] = b"GSXDB\0\0\x03";

/// A fully decoded snapshot body: balances + EVM contract state + bytes_state.
#[derive(Debug)]
struct DecodedSnapshot {
    balances: Vec<(Address, u128)>,
    codes: Vec<([u8; 32], Vec<u8>)>,
    storages: Vec<crate::EvmStorageEntry>,
    account_codes: Vec<(Address, [u8; 32])>,
    bytes: Vec<(Address, Vec<u8>)>,
}

/// Bounds-checked cursor over a snapshot body.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("snapshot length overflow")?;
        if end > self.buf.len() {
            return Err(format!(
                "snapshot truncated: need {n} bytes at offset {}",
                self.pos
            ));
        }
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    fn u32_len(&mut self) -> Result<usize, String> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }

    /// Bound an untrusted section count against the bytes left in the
    /// buffer before allocating, so a corrupt or malicious length field
    /// can't drive a huge `Vec::with_capacity` (a memory-amplification
    /// DoS) before the data is proven present. `min_entry` is the smallest
    /// possible encoded size of one entry in this section.
    fn checked_count(&self, n: usize, min_entry: usize, label: &str) -> Result<usize, String> {
        let remaining = self.buf.len() - self.pos;
        let max = remaining / min_entry.max(1);
        if n > max {
            return Err(format!(
                "snapshot {label} count {n} exceeds {max} possible in {remaining} remaining bytes"
            ));
        }
        Ok(n)
    }

    fn arr<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }
}

/// Decode a sectioned (v2) snapshot body. Validates the magic, every
/// section count, and rejects trailing bytes — the single source of truth
/// for both `restore_into_state` (which applies) and `read_from_file` /
/// `entry_count` (which only validate).
fn decode_snapshot_body(encoded: &[u8]) -> Result<DecodedSnapshot, String> {
    if encoded.len() < SNAPSHOT_MAGIC.len() || &encoded[..SNAPSHOT_MAGIC.len()] != SNAPSHOT_MAGIC {
        return Err(format!("snapshot magic mismatch (expected {SNAPSHOT_MAGIC:?})"));
    }
    let mut r = Reader {
        buf: encoded,
        pos: SNAPSHOT_MAGIC.len(),
    };

    let n = r.u32_len()?;
    let n = r.checked_count(n, 36, "balances")?; // addr(20) + bal(16)
    let mut balances = Vec::with_capacity(n);
    for _ in 0..n {
        let addr = Address(r.arr::<20>()?);
        let bal = u128::from_le_bytes(r.arr::<16>()?);
        balances.push((addr, bal));
    }

    let n = r.u32_len()?;
    let n = r.checked_count(n, 36, "codes")?; // hash(32) + len(4) + min code(0)
    let mut codes = Vec::with_capacity(n);
    for _ in 0..n {
        let hash = r.arr::<32>()?;
        let len = r.u32_len()?;
        let code = r.take(len)?.to_vec();
        // Validate `code_hash == keccak256(code)` at decode time so
        // a corrupt or malicious snapshot fails loud with a typed
        // error rather than panicking later in `State::apply`
        // (which enforces the same invariant on application). The
        // assertion in `apply` is the structural guarantee; this
        // is the friendly failure mode for the snapshot path.
        let computed: [u8; 32] = sha3::Keccak256::digest(&code).into();
        if hash != computed {
            return Err(format!(
                "snapshot code_hash mismatch: stored {:?}, computed keccak256 {:?}",
                hash, computed,
            ));
        }
        codes.push((hash, code));
    }

    let n = r.u32_len()?;
    let n = r.checked_count(n, 84, "storages")?; // addr(20) + slot(32) + value(32)
    let mut storages = Vec::with_capacity(n);
    for _ in 0..n {
        let addr = Address(r.arr::<20>()?);
        let slot = r.arr::<32>()?;
        let value = r.arr::<32>()?;
        storages.push(((addr, slot), value));
    }

    let n = r.u32_len()?;
    let n = r.checked_count(n, 52, "account_codes")?; // addr(20) + hash(32)
    let mut account_codes = Vec::with_capacity(n);
    for _ in 0..n {
        let addr = Address(r.arr::<20>()?);
        let hash = r.arr::<32>()?;
        account_codes.push((addr, hash));
    }

    let n = r.u32_len()?;
    let mut bytes = Vec::with_capacity(n);
    for _ in 0..n {
        let addr = Address(r.arr::<20>()?);
        let len = r.u32_len()?;
        bytes.push((addr, r.take(len)?.to_vec()));
    }

    if r.pos != encoded.len() {
        return Err(format!(
            "snapshot has {} trailing bytes",
            encoded.len() - r.pos
        ));
    }
    Ok(DecodedSnapshot {
        balances,
        codes,
        storages,
        account_codes,
        bytes,
    })
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
    ///
    /// **B4 audit note**: this freshness gate reads `SystemTime::now`
    /// at validation time. Local clock skew can reject otherwise-valid
    /// snapshots; operators with drift should widen `max_age_secs`.
    /// Not a soundness issue — snapshot equality / restore correctness
    /// do not depend on the timestamp, only this acceptance window.
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
        // Each section is sorted so the byte stream is independent of map /
        // store iteration order (idempotent round-trips, deterministic across
        // nodes).
        let mut balances = state.entries();
        balances.sort_by_key(|(addr, _)| addr.0);
        let mut codes = state.evm_code_entries();
        codes.sort_by_key(|(hash, _)| *hash);
        let mut storages = state.evm_storage_entries();
        storages.sort_by_key(|((addr, slot), _)| (addr.0, *slot));
        let mut account_codes = state.evm_account_code_entries();
        account_codes.sort_by_key(|(addr, _)| addr.0);
        // `bytes_state_entries` is already address-sorted (BTreeMap).
        let bytes = state.bytes_state_entries();

        let mut encoded = Vec::new();
        encoded.extend_from_slice(SNAPSHOT_MAGIC);

        encoded.extend_from_slice(&(balances.len() as u32).to_le_bytes());
        for (addr, slot) in &balances {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(&slot.canonical().to_le_bytes());
        }

        encoded.extend_from_slice(&(codes.len() as u32).to_le_bytes());
        for (hash, code) in &codes {
            encoded.extend_from_slice(hash);
            encoded.extend_from_slice(&(code.len() as u32).to_le_bytes());
            encoded.extend_from_slice(code);
        }

        encoded.extend_from_slice(&(storages.len() as u32).to_le_bytes());
        for ((addr, slot), value) in &storages {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(slot);
            encoded.extend_from_slice(value);
        }

        encoded.extend_from_slice(&(account_codes.len() as u32).to_le_bytes());
        for (addr, hash) in &account_codes {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(hash);
        }

        encoded.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        for (addr, b) in &bytes {
            encoded.extend_from_slice(&addr.0);
            encoded.extend_from_slice(&(b.len() as u32).to_le_bytes());
            encoded.extend_from_slice(b);
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
        // Structural validation — caught here so callers don't have to
        // pre-validate before `restore_into_state`. Decodes (and discards)
        // so a corrupt file (bad magic, short section, trailing bytes)
        // fails loud at read time.
        decode_snapshot_body(&snapshot.encoded_state)?;
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
        let decoded = decode_snapshot_body(&self.encoded_state)?;
        let mut applied = 0usize;
        for (addr, value) in &decoded.balances {
            state.apply(
                token,
                &StateChange::SetBalance {
                    addr: *addr,
                    to: Balance(*value),
                },
            );
            applied += 1;
        }
        // Code before account-code so the pointer always resolves to bytes.
        for (code_hash, code) in &decoded.codes {
            state.apply(
                token,
                &StateChange::SetCode {
                    code_hash: *code_hash,
                    code: code.clone(),
                },
            );
            applied += 1;
        }
        for ((addr, slot), value) in &decoded.storages {
            state.apply(
                token,
                &StateChange::SetStorage {
                    addr: *addr,
                    slot: *slot,
                    value: *value,
                },
            );
            applied += 1;
        }
        for (addr, code_hash) in &decoded.account_codes {
            state.apply(
                token,
                &StateChange::SetAccountCode {
                    addr: *addr,
                    code_hash: *code_hash,
                },
            );
            applied += 1;
        }
        for (addr, b) in &decoded.bytes {
            state.apply(
                token,
                &StateChange::SetBytes {
                    addr: *addr,
                    bytes: b.clone(),
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
        decode_snapshot_body(&self.encoded_state)
            .ok()
            .map(|d| d.balances.len())
    }

    /// Unused stub kept until `BalanceSlot` is reachable directly.
    #[doc(hidden)]
    pub fn __load_balance_slot_for_tests(byte: u8) -> BalanceSlot {
        BalanceSlot::new(u128::from(byte))
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
        assert!(err.contains("truncated"), "got: {err}");
    }

    #[test]
    fn snapshot_round_trips_evm_contract_state() {
        // A snapshot must capture contract code + storage + account-code,
        // not just balances — otherwise a restored node loses contract state
        // and its combined state_root diverges from the pre-snapshot root.
        use sha3::{Digest, Keccak256};
        let mut original = State::default();
        let token = BridgeToken::__for_bridge_only();
        let contract = Address([9; 20]);
        let code = vec![0x60u8, 0x00, 0x55];
        // SetCode enforces `code_hash == keccak256(code)`; compute the real
        // hash so the snapshot path doesn't trip the invariant on restore.
        let code_hash: [u8; 32] = Keccak256::digest(&code).into();
        original.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([1; 20]),
                to: Balance(500),
            },
        );
        original.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code: code.clone(),
            },
        );
        original.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash,
            },
        );
        original.apply(
            &token,
            &StateChange::SetStorage {
                addr: contract,
                slot: [1u8; 32],
                value: [0xAB; 32],
            },
        );

        let root_before = original.state_root();
        let snapshot = StateSnapshot::from_state(&original, 5, None);

        let mut restored = State::default();
        snapshot
            .restore_into_state(&mut restored, &token)
            .expect("restore");

        assert_eq!(
            restored.code_by_hash(&code_hash),
            Some([0x60, 0x00, 0x55].as_slice())
        );
        assert_eq!(restored.account_code_hash(&contract), Some(code_hash));
        assert_eq!(restored.storage_at(&contract, &[1u8; 32]), [0xAB; 32]);
        assert_eq!(restored.balance_of(&Address([1; 20])), Balance(500));
        // The combined root matches — contract state is fully captured.
        assert_eq!(restored.state_root(), root_before);
    }

    /// Codex P2 (lib.rs:369) — friendly failure path: a snapshot
    /// whose encoded `(code_hash, code)` pair doesn't satisfy
    /// `code_hash == keccak256(code)` is rejected at decode time
    /// with a typed error, rather than panicking in `State::apply`
    /// mid-restore. The application-time assert is the structural
    /// guarantee; this is the importable-snapshot ergonomics.
    #[test]
    fn snapshot_decode_rejects_mismatched_code_hash() {
        // Hand-craft an encoded snapshot whose code section has a
        // hash that doesn't match the code bytes. Sections: balances
        // (0), codes (1), storages (0), account_codes (0).
        let mut buf = Vec::new();
        buf.extend_from_slice(SNAPSHOT_MAGIC);
        buf.extend_from_slice(&0u32.to_le_bytes()); // balances len
        buf.extend_from_slice(&1u32.to_le_bytes()); // codes len
        buf.extend_from_slice(&[0u8; 32]); // wrong hash
        let code = vec![0x60u8, 0x00, 0x55];
        buf.extend_from_slice(&(code.len() as u32).to_le_bytes());
        buf.extend_from_slice(&code);
        buf.extend_from_slice(&0u32.to_le_bytes()); // storages len
        buf.extend_from_slice(&0u32.to_le_bytes()); // account_codes len

        let err = decode_snapshot_body(&buf).expect_err("bad code_hash must reject");
        assert!(
            err.contains("code_hash mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn snapshot_round_trips_bytes_state() {
        // A snapshot must capture the reserved-address bytes registry (L2
        // verifying keys / DA anchors / governance) so a restored node's
        // combined root matches the pre-snapshot root.
        let mut original = State::default();
        let token = BridgeToken::__for_bridge_only();
        let key_addr = Address([5; 20]);
        original.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([1; 20]),
                to: Balance(42),
            },
        );
        original.apply(
            &token,
            &StateChange::SetBytes {
                addr: key_addr,
                bytes: vec![0xDE, 0xAD, 0xBE, 0xEF],
            },
        );
        let root_before = original.state_root();

        let snapshot = StateSnapshot::from_state(&original, 7, None);
        let mut restored = State::default();
        snapshot
            .restore_into_state(&mut restored, &token)
            .expect("restore");

        assert_eq!(
            restored.read_bytes(&key_addr),
            Some([0xDE, 0xAD, 0xBE, 0xEF].as_slice())
        );
        assert_eq!(restored.state_root(), root_before);
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
