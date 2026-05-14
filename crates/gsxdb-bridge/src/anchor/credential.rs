//! Asymmetric anchor authentication credentials.
//!
//! `AuthScheme::Blake3Mac` carries its 32-byte MAC inline in
//! [`Anchor::mac`](super::types::Anchor::mac). Real signatures
//! (ECDSA secp256k1 — 65 bytes recoverable; ML-DSA-65 — ~3.3 KB) do
//! not fit there, so they ride alongside the anchor in this credential
//! envelope.
//!
//! Per IQ-7, the launch verifier is a hybrid ECDSA secp256k1 +
//! ML-DSA-65 AND-gate against the `LTPAnchorRegistry.sol` authorized
//! signer set. Step B of Track 1.2 wires the ECDSA half; Steps C–D
//! wire ML-DSA-65 and the hybrid composition.
//!
//! # Solidity parity
//!
//! The ECDSA signing payload reproduces
//! `LTPAnchorRegistry.recoverSigner`:
//!
//! ```text
//! inner = keccak256(abi.encode(chainId, height, stateRoot, parent, mac))
//! payload = keccak256("\x19Ethereum Signed Message:\n32" || inner)
//! ```
//!
//! `abi.encode` for a struct of all-static fields is each field padded
//! left-zero to 32 bytes and concatenated, in declaration order. The
//! Solidity `Anchor` struct intentionally omits the Rust `auth_scheme`
//! discriminant — the payload binds only the on-chain fields.

use super::types::{Anchor, AnchorHash, ChainId};
use gsxdb_state::Commitment;
use k256::ecdsa::{RecoveryId, Signature, VerifyingKey};
use sha3::{Digest, Keccak256};

#[cfg(feature = "production-pqc")]
use pqcrypto_mldsa::mldsa65;
#[cfg(feature = "production-pqc")]
use pqcrypto_traits::sign::{
    DetachedSignature as _, PublicKey as _, SecretKey as _, VerificationError,
};

/// Length of a recoverable secp256k1 signature: r (32) || s (32) || v (1).
pub const ECDSA_SIG_LEN: usize = 65;

/// Sidecar credential carrying real signature bytes alongside an
/// [`Anchor`]. Variants are 1:1 with
/// [`AuthScheme`](super::types::AuthScheme).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorAuthCredential {
    /// `Blake3Mac` carries its MAC inline in [`Anchor::mac`] — no
    /// sidecar bytes needed. This variant exists so dispatch is
    /// total over all schemes.
    Blake3Mac,
    /// Reserved for `Sp1ZkProof` (Track 1.3). Payload schema is TBD.
    Sp1ZkProof {
        /// Opaque zk-proof bytes; schema decided when the verifier lands.
        proof: Vec<u8>,
    },
    /// 65-byte recoverable ECDSA signature (r || s || v) over the
    /// EIP-191-prefixed `abi.encode(anchor)` digest. Matches what
    /// `LTPAnchorRegistry.acceptAnchor` consumes.
    EcdsaSecp256k1 {
        /// `r || s || v`. `v ∈ {27, 28}` per Ethereum convention.
        signature: [u8; ECDSA_SIG_LEN],
    },
    /// Hybrid: both ECDSA and ML-DSA-65 signatures over the same
    /// payload. Both must verify. ML-DSA-65 wiring pending Step C of
    /// Track 1.2.
    MlDsa65Hybrid {
        /// 65-byte recoverable ECDSA signature, same format as
        /// [`Self::EcdsaSecp256k1`].
        ecdsa_signature: [u8; ECDSA_SIG_LEN],
        /// ML-DSA-65 signature bytes (~3.3 KiB) per FIPS 204.
        mldsa_signature: Vec<u8>,
    },
}

/// Reasons an ECDSA verification can fail. Distinct from
/// [`AuthVerifyError`](super::types::AuthVerifyError) so the AND-gate
/// composer in Step D can surface per-component failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EcdsaVerifyError {
    /// Signature byte layout was wrong (length, malformed scalars,
    /// or recovery id out of range).
    MalformedSignature,
    /// Signature parsed but did not recover a public key.
    RecoveryFailed,
    /// Recovered signer's Ethereum address did not match the
    /// expected approved signer.
    UnauthorizedSigner,
}

/// 20-byte Ethereum address. Matches Solidity `address`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EthAddress(pub [u8; 20]);

impl EthAddress {
    /// Derive the Ethereum address from a secp256k1 verifying key:
    /// the rightmost 20 bytes of keccak256 over the uncompressed
    /// public key with the leading 0x04 stripped.
    #[must_use]
    pub fn from_verifying_key(vk: &VerifyingKey) -> Self {
        let encoded = vk.to_encoded_point(false);
        let bytes = encoded.as_bytes();
        // First byte is the SEC1 tag (0x04 for uncompressed); the
        // remaining 64 bytes are X||Y.
        debug_assert_eq!(bytes.len(), 65);
        debug_assert_eq!(bytes[0], 0x04);
        let digest = Keccak256::digest(&bytes[1..]);
        let mut out = [0u8; 20];
        out.copy_from_slice(&digest[12..]);
        EthAddress(out)
    }
}

/// `abi.encode(Anchor)` — each static field padded left-zero to 32
/// bytes and concatenated in declaration order. The Solidity struct
/// has five fields: `chainId, height, stateRoot, parent, mac`.
/// The on-chain struct deliberately omits Rust's `auth_scheme`.
#[must_use]
pub fn solidity_abi_encode(
    chain_id: ChainId,
    height: u64,
    state_root: &Commitment,
    parent: &AnchorHash,
    mac: &[u8; 32],
) -> [u8; 160] {
    let mut buf = [0u8; 160];
    // chainId: uint32 padded to 32 bytes (big-endian, right-aligned)
    buf[28..32].copy_from_slice(&chain_id.0.to_be_bytes());
    // height: uint64 padded to 32 bytes
    buf[56..64].copy_from_slice(&height.to_be_bytes());
    // stateRoot, parent, mac: each already 32 bytes
    buf[64..96].copy_from_slice(&state_root.0);
    buf[96..128].copy_from_slice(&parent.0);
    buf[128..160].copy_from_slice(mac);
    buf
}

/// Compute the EIP-191 message hash the Solidity contract recovers
/// against. Matches `LTPAnchorRegistry.recoverSigner` exactly.
#[must_use]
pub fn eth_signed_message_hash(anchor: &Anchor) -> [u8; 32] {
    let encoded = solidity_abi_encode(
        anchor.chain_id,
        anchor.height,
        &anchor.state_root,
        &anchor.parent,
        &anchor.mac,
    );
    let inner = Keccak256::digest(encoded);

    let mut h = Keccak256::new();
    h.update(b"\x19Ethereum Signed Message:\n32");
    h.update(inner);
    let mut out = [0u8; 32];
    out.copy_from_slice(&h.finalize());
    out
}

/// Reasons an ML-DSA-65 verification can fail.
#[cfg(feature = "production-pqc")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MlDsaVerifyError {
    /// Signature bytes did not parse as an ML-DSA-65 detached signature.
    MalformedSignature,
    /// Signature parsed but did not verify under the supplied public key.
    InvalidSignature,
}

/// Verify an ML-DSA-65 detached signature over the same EIP-191 anchor
/// payload that ECDSA signs. Using a shared payload lets the AND-gate
/// composer (Step D) call both verifiers without recomputing.
///
/// # Errors
///
/// Returns [`MlDsaVerifyError`] when the signature is malformed or
/// does not verify under `public_key`.
#[cfg(feature = "production-pqc")]
pub fn verify_mldsa65(
    anchor: &Anchor,
    signature_bytes: &[u8],
    public_key_bytes: &[u8],
) -> Result<(), MlDsaVerifyError> {
    let sig = mldsa65::DetachedSignature::from_bytes(signature_bytes)
        .map_err(|_| MlDsaVerifyError::MalformedSignature)?;
    let pk = mldsa65::PublicKey::from_bytes(public_key_bytes)
        .map_err(|_| MlDsaVerifyError::MalformedSignature)?;
    let payload = eth_signed_message_hash(anchor);
    match mldsa65::verify_detached_signature(&sig, &payload, &pk) {
        Ok(()) => Ok(()),
        Err(VerificationError::InvalidSignature) => Err(MlDsaVerifyError::InvalidSignature),
        Err(_) => Err(MlDsaVerifyError::MalformedSignature),
    }
}

/// Verify an ECDSA secp256k1 signature against the EIP-191 anchor
/// payload and check the recovered Ethereum address matches
/// `expected_signer`.
///
/// The signature layout is `r || s || v` (65 bytes), with `v` in
/// `{27, 28}` per Ethereum convention. Pre-EIP-155 only — chain-id-
/// folded recovery is not needed here because the contract recovers
/// from the EIP-191 message hash, not a transaction.
///
/// # Errors
///
/// Returns [`EcdsaVerifyError`] when the sig is malformed, recovery
/// fails, or the recovered address is not the expected signer.
pub fn verify_ecdsa(
    anchor: &Anchor,
    signature: &[u8; ECDSA_SIG_LEN],
    expected_signer: &EthAddress,
) -> Result<(), EcdsaVerifyError> {
    let v = signature[64];
    let recovery_byte = match v {
        27 => 0u8,
        28 => 1u8,
        _ => return Err(EcdsaVerifyError::MalformedSignature),
    };
    let recovery_id =
        RecoveryId::from_byte(recovery_byte).ok_or(EcdsaVerifyError::MalformedSignature)?;
    let sig = Signature::try_from(&signature[..64])
        .map_err(|_| EcdsaVerifyError::MalformedSignature)?;

    let payload = eth_signed_message_hash(anchor);
    let vk = VerifyingKey::recover_from_prehash(&payload, &sig, recovery_id)
        .map_err(|_| EcdsaVerifyError::RecoveryFailed)?;

    let recovered = EthAddress::from_verifying_key(&vk);
    if recovered == *expected_signer {
        Ok(())
    } else {
        Err(EcdsaVerifyError::UnauthorizedSigner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::anchor::types::{AuthScheme, GENESIS_PARENT};
    use k256::ecdsa::{signature::hazmat::PrehashSigner, SigningKey};
    use rand::rngs::OsRng;

    fn sample_anchor() -> Anchor {
        Anchor {
            chain_id: ChainId(103_115_120),
            height: 42,
            state_root: Commitment([7u8; 32]),
            parent: GENESIS_PARENT,
            mac: [0u8; 32],
            auth_scheme: AuthScheme::EcdsaSecp256k1,
        }
    }

    fn sign_anchor(sk: &SigningKey, anchor: &Anchor) -> [u8; ECDSA_SIG_LEN] {
        let payload = eth_signed_message_hash(anchor);
        let (sig, rec) = sk
            .sign_prehash(&payload)
            .expect("sign succeeds for valid prehash");
        // k256 returns rec = 0 or 1; Ethereum wants v = 27 + rec.
        let mut bytes = [0u8; ECDSA_SIG_LEN];
        bytes[..64].copy_from_slice(&sig.to_bytes());
        bytes[64] = 27 + rec.to_byte();
        bytes
    }

    #[test]
    fn solidity_abi_encode_is_160_bytes_with_field_offsets() {
        let bytes = solidity_abi_encode(
            ChainId(1),
            2,
            &Commitment([0xaa; 32]),
            &AnchorHash([0xbb; 32]),
            &[0xcc; 32],
        );
        assert_eq!(bytes.len(), 160);
        // chainId in the last 4 bytes of word 0
        assert_eq!(&bytes[28..32], &1u32.to_be_bytes());
        // height in the last 8 bytes of word 1
        assert_eq!(&bytes[56..64], &2u64.to_be_bytes());
        // state_root at word 2
        assert_eq!(&bytes[64..96], &[0xaa; 32]);
        // parent at word 3
        assert_eq!(&bytes[96..128], &[0xbb; 32]);
        // mac at word 4
        assert_eq!(&bytes[128..160], &[0xcc; 32]);
    }

    #[test]
    fn ecdsa_roundtrip_recovers_signer() {
        let sk = SigningKey::random(&mut OsRng);
        let vk = sk.verifying_key();
        let signer = EthAddress::from_verifying_key(vk);

        let anchor = sample_anchor();
        let sig = sign_anchor(&sk, &anchor);

        verify_ecdsa(&anchor, &sig, &signer).expect("roundtrip verifies");
    }

    #[test]
    fn ecdsa_rejects_tampered_state_root() {
        let sk = SigningKey::random(&mut OsRng);
        let signer = EthAddress::from_verifying_key(sk.verifying_key());

        let original = sample_anchor();
        let sig = sign_anchor(&sk, &original);

        let mut tampered = original;
        tampered.state_root = Commitment([0xff; 32]);
        let err = verify_ecdsa(&tampered, &sig, &signer).unwrap_err();
        // Tampering changes the payload, which recovers a *different*
        // address (or fails recovery entirely).
        assert!(matches!(
            err,
            EcdsaVerifyError::UnauthorizedSigner | EcdsaVerifyError::RecoveryFailed
        ));
    }

    #[test]
    fn ecdsa_rejects_wrong_signer() {
        let sk = SigningKey::random(&mut OsRng);
        let wrong_signer = EthAddress([0u8; 20]);

        let anchor = sample_anchor();
        let sig = sign_anchor(&sk, &anchor);

        assert_eq!(
            verify_ecdsa(&anchor, &sig, &wrong_signer),
            Err(EcdsaVerifyError::UnauthorizedSigner)
        );
    }

    #[test]
    fn ecdsa_rejects_malformed_v_byte() {
        let sk = SigningKey::random(&mut OsRng);
        let signer = EthAddress::from_verifying_key(sk.verifying_key());

        let anchor = sample_anchor();
        let mut sig = sign_anchor(&sk, &anchor);
        sig[64] = 42; // not in {27, 28}

        assert_eq!(
            verify_ecdsa(&anchor, &sig, &signer),
            Err(EcdsaVerifyError::MalformedSignature)
        );
    }

    #[test]
    fn eth_signed_message_hash_is_deterministic() {
        let a = sample_anchor();
        assert_eq!(eth_signed_message_hash(&a), eth_signed_message_hash(&a));
    }

    #[cfg(feature = "production-pqc")]
    mod mldsa {
        use super::super::*;
        use super::sample_anchor;
        use pqcrypto_mldsa::mldsa65;
        use pqcrypto_traits::sign::DetachedSignature as _;
        use pqcrypto_traits::sign::PublicKey as _;

        fn sign_anchor_mldsa(
            sk: &mldsa65::SecretKey,
            anchor: &Anchor,
        ) -> mldsa65::DetachedSignature {
            let payload = eth_signed_message_hash(anchor);
            mldsa65::detached_sign(&payload, sk)
        }

        #[test]
        fn mldsa_roundtrip_verifies() {
            let (pk, sk) = mldsa65::keypair();
            let anchor = sample_anchor();
            let sig = sign_anchor_mldsa(&sk, &anchor);

            verify_mldsa65(&anchor, sig.as_bytes(), pk.as_bytes())
                .expect("roundtrip verifies");
        }

        #[test]
        fn mldsa_rejects_tampered_payload() {
            let (pk, sk) = mldsa65::keypair();
            let original = sample_anchor();
            let sig = sign_anchor_mldsa(&sk, &original);

            let mut tampered = original;
            tampered.state_root = gsxdb_state::Commitment([0xff; 32]);
            assert_eq!(
                verify_mldsa65(&tampered, sig.as_bytes(), pk.as_bytes()),
                Err(MlDsaVerifyError::InvalidSignature)
            );
        }

        #[test]
        fn mldsa_rejects_wrong_public_key() {
            let (_pk_real, sk) = mldsa65::keypair();
            let (pk_other, _sk_other) = mldsa65::keypair();
            let anchor = sample_anchor();
            let sig = sign_anchor_mldsa(&sk, &anchor);

            assert_eq!(
                verify_mldsa65(&anchor, sig.as_bytes(), pk_other.as_bytes()),
                Err(MlDsaVerifyError::InvalidSignature)
            );
        }

        #[test]
        fn mldsa_rejects_garbage_signature() {
            // Whether pqcrypto-mldsa fails at parse time
            // (MalformedSignature) or only at verify time
            // (InvalidSignature) is an implementation detail of the
            // upstream crate; the load-bearing property is that the
            // garbage input is rejected.
            let (pk, _sk) = mldsa65::keypair();
            let anchor = sample_anchor();
            let bad_sig = [0u8; 16];

            assert!(matches!(
                verify_mldsa65(&anchor, &bad_sig, pk.as_bytes()),
                Err(MlDsaVerifyError::MalformedSignature)
                    | Err(MlDsaVerifyError::InvalidSignature)
            ));
        }
    }

    #[test]
    fn eth_signed_message_hash_changes_with_each_field() {
        let base = sample_anchor();
        let h0 = eth_signed_message_hash(&base);
        let base_height = base.height;

        let mut a = base.clone();
        a.chain_id = ChainId(2);
        assert_ne!(h0, eth_signed_message_hash(&a));

        let mut a = base.clone();
        a.height = base_height + 1;
        assert_ne!(h0, eth_signed_message_hash(&a));

        let mut a = base.clone();
        a.state_root = Commitment([1u8; 32]);
        assert_ne!(h0, eth_signed_message_hash(&a));

        let mut a = base.clone();
        a.parent = AnchorHash([1u8; 32]);
        assert_ne!(h0, eth_signed_message_hash(&a));

        let mut a = base;
        a.mac = [1u8; 32];
        assert_ne!(h0, eth_signed_message_hash(&a));
    }
}
