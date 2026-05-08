//! Anchor data types.

use blake3::Hasher;
use gsxdb_state::Commitment;

/// Identifier for an anchor target chain. Phase-1 phase-1 uses small
/// integers; real-deploy might use the EVM `chainid` (u64) or a custom
/// registry index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChainId(pub u32);

/// 32-byte anchor hash. BLAKE3 of the anchor's canonical encoding.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct AnchorHash(pub [u8; 32]);

impl std::fmt::Debug for AnchorHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AnchorHash(0x")?;
        for b in &self.0[..4] {
            write!(f, "{b:02x}")?;
        }
        write!(f, "..)")
    }
}

/// Genesis parent — used as the `parent` field of the first anchor on
/// any chain.
pub const GENESIS_PARENT: AnchorHash = AnchorHash([0; 32]);

/// One per-chain anchor for one block.
///
/// Encodes a chain's commitment to a `(height, state_root)` pair plus
/// a back-pointer to the previous anchor on that chain. The MAC binds
/// these fields under the chain's authenticator key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// Target chain.
    pub chain_id: ChainId,
    /// Logical block height. Same logical height across all chains
    /// for a given source block.
    pub height: u64,
    /// State-tree root commitment from S6.
    pub state_root: Commitment,
    /// Hash of the previous anchor on this chain. [`GENESIS_PARENT`]
    /// for the first anchor.
    pub parent: AnchorHash,
    /// MAC over (`chain_id` | height | `state_root` | parent) under the
    /// chain's key.
    pub mac: [u8; 32],
}

impl Anchor {
    /// Construct an anchor and compute its MAC. The MAC is BLAKE3
    /// keyed-hash of the canonical encoding under `key`.
    #[must_use]
    pub fn new(
        chain_id: ChainId,
        height: u64,
        state_root: Commitment,
        parent: AnchorHash,
        key: &[u8; 32],
    ) -> Self {
        let mac = compute_mac(chain_id, height, &state_root, &parent, key);
        Self {
            chain_id,
            height,
            state_root,
            parent,
            mac,
        }
    }

    /// Verify the MAC under `key`. Returns `true` iff the recomputed
    /// MAC matches.
    #[must_use]
    pub fn verify_mac(&self, key: &[u8; 32]) -> bool {
        let expected = compute_mac(
            self.chain_id,
            self.height,
            &self.state_root,
            &self.parent,
            key,
        );
        // Constant-time-ish comparison. BLAKE3 outputs are fixed
        // length so a simple `==` is fine here, but a real deploy
        // would use a CT comparator.
        self.mac == expected
    }

    /// Hash of this anchor. Used as the `parent` field of the next
    /// anchor on the same chain.
    #[must_use]
    pub fn hash(&self) -> AnchorHash {
        let mut h = Hasher::new();
        h.update(b"GSXDB-ANCHOR/HASH");
        h.update(&self.chain_id.0.to_be_bytes());
        h.update(&self.height.to_be_bytes());
        h.update(&self.state_root.0);
        h.update(&self.parent.0);
        h.update(&self.mac);
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
        AnchorHash(out)
    }
}

fn compute_mac(
    chain_id: ChainId,
    height: u64,
    state_root: &Commitment,
    parent: &AnchorHash,
    key: &[u8; 32],
) -> [u8; 32] {
    // BLAKE3 keyed-hash mode: built-in MAC primitive. Domain-separated
    // by tag to avoid cross-context collisions.
    let mut h = Hasher::new_keyed(key);
    h.update(b"GSXDB-ANCHOR/MAC");
    h.update(&chain_id.0.to_be_bytes());
    h.update(&height.to_be_bytes());
    h.update(&state_root.0);
    h.update(&parent.0);
    let mut out = [0u8; 32];
    out.copy_from_slice(h.finalize().as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> [u8; 32] {
        [7; 32]
    }
    fn root(byte: u8) -> Commitment {
        Commitment([byte; 32])
    }

    #[test]
    fn mac_round_trips_under_correct_key() {
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        assert!(a.verify_mac(&key()));
    }

    #[test]
    fn mac_rejects_wrong_key() {
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        assert!(!a.verify_mac(&[8; 32]));
    }

    #[test]
    fn mac_rejects_tampered_state_root() {
        let mut a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        a.state_root = root(2);
        assert!(!a.verify_mac(&key()));
    }

    #[test]
    fn mac_rejects_tampered_height() {
        let mut a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        a.height = 1;
        assert!(!a.verify_mac(&key()));
    }

    #[test]
    fn distinct_inputs_distinct_hashes() {
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        let b = Anchor::new(ChainId(1), 1, root(1), GENESIS_PARENT, &key());
        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn same_inputs_same_hashes() {
        let a = Anchor::new(ChainId(2), 5, root(7), AnchorHash([42; 32]), &key());
        let b = Anchor::new(ChainId(2), 5, root(7), AnchorHash([42; 32]), &key());
        assert_eq!(a.hash(), b.hash());
        assert_eq!(a.mac, b.mac);
    }

    #[test]
    fn distinct_chain_ids_distinct_macs_under_same_key() {
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        let b = Anchor::new(ChainId(2), 0, root(1), GENESIS_PARENT, &key());
        assert_ne!(a.mac, b.mac);
    }

    #[test]
    fn anchor_hash_debug_is_hex_prefix() {
        let h = AnchorHash([0xab; 32]);
        let s = format!("{h:?}");
        assert!(s.contains("ab"));
        assert!(s.starts_with("AnchorHash(0x"));
    }
}
