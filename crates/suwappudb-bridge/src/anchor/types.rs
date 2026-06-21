//! Anchor data types.

use blake3::Hasher;
use suwappudb_state::Commitment;

/// Identifier for an anchor target chain. Phase-1 phase-1 uses small
/// integers; real-deploy might use the EVM `chainid` (u64) or a custom
/// registry index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ChainId(pub u32);

/// Authentication scheme carried by an anchor.
///
/// Phase-1 uses keyed BLAKE3 MACs. The launch surface (S11) is the
/// `LTPAnchorRegistry.sol` contract, which accepts a hybrid
/// ECDSA secp256k1 + ML-DSA-65 signature per
/// [IQ-7](../../../docs/iq/IQ-7-anchor-parity.md) §"Authentication".
/// Either component breaking does not compromise the other.
///
/// New variants are additive — the wire encoding via `#[repr(u8)]`
/// is fixed by discriminant order, so existing variants must keep
/// their position.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthScheme {
    /// Keyed BLAKE3 MAC (phase-1 substrate).
    Blake3Mac = 0,
    /// Proof verified through an SP1-compatible zk circuit. Reserved
    /// for the validity-proof path (Track 1.3); no verifier wired yet.
    Sp1ZkProof = 1,
    /// ECDSA over secp256k1. The launch-L1 path: signatures recovered
    /// against the registry's authorized signer per `LTPAnchorRegistry.sol`.
    /// No verifier wired yet — pending S11 ECDSA dispatch (Track 1.2 Step B).
    EcdsaSecp256k1 = 2,
    /// Hybrid ECDSA secp256k1 + ML-DSA-65. The post-quantum surface:
    /// both signatures must verify (AND gate) for the anchor to be
    /// accepted. No verifier wired yet — pending S11 hybrid dispatch
    /// (Track 1.2 Steps C–D).
    MlDsa65Hybrid = 3,
}

/// Detailed anchor-auth verification result for scheme dispatch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthVerifyError {
    /// The configured scheme has no verifier wired yet.
    UnsupportedScheme(AuthScheme),
    /// The authenticator bytes failed verification.
    InvalidAuthenticator,
}

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
    /// Authenticator over (`chain_id` | height | `state_root` | parent).
    ///
    /// For [`AuthScheme::Blake3Mac`] this is the keyed MAC output.
    /// For other schemes this field is a compact commitment to external
    /// proof/signature bytes managed by launch-readiness components.
    pub mac: [u8; 32],
    /// Authentication mode used by this anchor.
    pub auth_scheme: AuthScheme,
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
            auth_scheme: AuthScheme::Blake3Mac,
        }
    }

    /// **S11.3** — Construct an ECDSA-scheme anchor. The `mac` field is
    /// zero-filled (no MAC for non-Blake3 schemes); the actual ECDSA
    /// signature lives in the sidecar
    /// [`super::credential::AnchorAuthCredential::EcdsaSecp256k1`] produced
    /// by an `AnchorSigner`. The Solidity `hashAnchor` ABI-encodes the
    /// zero-`mac` field as 32 zero-bytes, so this matches on-chain
    /// parity exactly.
    #[must_use]
    pub fn ecdsa(
        chain_id: ChainId,
        height: u64,
        state_root: Commitment,
        parent: AnchorHash,
    ) -> Self {
        Self {
            chain_id,
            height,
            state_root,
            parent,
            mac: [0u8; 32],
            auth_scheme: AuthScheme::EcdsaSecp256k1,
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
        self.auth_scheme == AuthScheme::Blake3Mac && self.mac == expected
    }

    /// Verify the anchor authenticator for the configured scheme.
    ///
    /// For phase-1 this currently supports only [`AuthScheme::Blake3Mac`].
    /// Other modes are reserved for launch-readiness verifier plumbing.
    #[must_use]
    pub fn verify_auth(&self, key: &[u8; 32]) -> bool {
        self.verify_auth_result(key).is_ok()
    }

    /// Verify the anchor authenticator for the configured scheme and return
    /// structured errors for callers that need diagnostics.
    pub fn verify_auth_result(&self, key: &[u8; 32]) -> Result<(), AuthVerifyError> {
        match self.auth_scheme {
            AuthScheme::Blake3Mac => {
                if self.verify_mac(key) {
                    Ok(())
                } else {
                    Err(AuthVerifyError::InvalidAuthenticator)
                }
            }
            AuthScheme::Sp1ZkProof
            | AuthScheme::EcdsaSecp256k1
            | AuthScheme::MlDsa65Hybrid => Err(AuthVerifyError::UnsupportedScheme(self.auth_scheme)),
        }
    }

    /// Hash of this anchor. Used as the `parent` field of the next
    /// anchor on the same chain.
    ///
    /// Field set matches `LTPAnchorRegistry.hashAnchor` exactly (chainId,
    /// height, stateRoot, parent, mac) so chain linkage stays consistent
    /// between sides when both maintain anchor logs. The hash primitive
    /// differs (BLAKE3 here vs keccak256 on-chain) because each side
    /// maintains its own per-chain linkage — see
    /// `docs/iq/IQ-7-anchor-parity.md` for the parity model.
    ///
    /// `auth_scheme` is intentionally NOT folded into the digest: the
    /// Solidity struct doesn't carry it, and adding it would force any
    /// non-Blake3Mac anchor to produce a Rust-side parent hash that
    /// no on-chain mirror could reproduce. Parity-checker flagged this
    /// in PR #4 (IQ-7 hybrid auth) review as load-bearing once
    /// `AuthScheme` gained variants beyond `Blake3Mac`.
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
    fn verify_auth_rejects_non_mac_modes_for_now() {
        let mut a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        for scheme in [
            AuthScheme::Sp1ZkProof,
            AuthScheme::EcdsaSecp256k1,
            AuthScheme::MlDsa65Hybrid,
        ] {
            a.auth_scheme = scheme;
            assert!(
                !a.verify_auth(&key()),
                "scheme {scheme:?} must reject until S11 verifier lands"
            );
            assert_eq!(
                a.verify_auth_result(&key()),
                Err(AuthVerifyError::UnsupportedScheme(scheme)),
                "scheme {scheme:?} must return UnsupportedScheme",
            );
        }
    }

    #[test]
    fn verify_auth_result_reports_unsupported_scheme() {
        let mut a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        a.auth_scheme = AuthScheme::Sp1ZkProof;
        assert_eq!(
            a.verify_auth_result(&key()),
            Err(AuthVerifyError::UnsupportedScheme(AuthScheme::Sp1ZkProof))
        );
    }

    #[test]
    fn auth_scheme_discriminants_are_stable() {
        // Wire encoding for AuthScheme is fixed by discriminant order;
        // changing these silently would break LTPAnchorRegistry parity.
        // See docs/iq/IQ-7-anchor-parity.md.
        assert_eq!(AuthScheme::Blake3Mac as u8, 0);
        assert_eq!(AuthScheme::Sp1ZkProof as u8, 1);
        assert_eq!(AuthScheme::EcdsaSecp256k1 as u8, 2);
        assert_eq!(AuthScheme::MlDsa65Hybrid as u8, 3);
    }

    #[test]
    fn verify_auth_accepts_blake3_mac_mode() {
        let a = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &key());
        assert!(a.verify_auth(&key()));
    }

    #[test]
    fn anchor_hash_debug_is_hex_prefix() {
        let h = AnchorHash([0xab; 32]);
        let s = format!("{h:?}");
        assert!(s.contains("ab"));
        assert!(s.starts_with("AnchorHash(0x"));
    }
}
