//! **S11.3 producer pipeline.** Sign anchors so the dispatcher can
//! emit non-Blake3-MAC credentials.
//!
//! The [`AnchorSigner`] trait abstracts over scheme — implementations
//! produce an [`AnchorAuthCredential`] for a given anchor. The S11.3
//! shipped impl is [`EcdsaSecp256k1Signer`], wrapping a `k256`
//! [`SigningKey`]; it signs the EIP-191-prefixed `keccak256` of
//! `abi.encode(anchor)` exactly as `LTPAnchorRegistry.recoverSigner`
//! consumes it.
//!
//! Future:
//!
//! - `Sp1ZkProofSigner` — assembles `Sp1PublicValues` + opaque proof
//!   bytes once the zkVM toolchain decision lands (Track 1.3 Step 2).
//! - `MlDsa65HybridSigner` — produces both halves of the AND-gate when
//!   the bridge is built with `production-pqc`.

use super::credential::{eth_signed_message_hash, AnchorAuthCredential, EthAddress, ECDSA_SIG_LEN};
use super::types::{Anchor, AuthScheme};
use k256::ecdsa::{RecoveryId, Signature, SigningKey};

/// Producer-side counterpart to the verifier-side
/// [`super::credential::VerifierConfig`]. Implementations sign anchors
/// they produce; the dispatcher pairs each signer with a chain so the
/// emitted credential matches the chain's registered config.
pub trait AnchorSigner {
    /// Scheme this signer produces. The dispatcher cross-checks this
    /// against the chain's [`super::credential::VerifierConfig::scheme`]
    /// before signing — mismatched signer/config is an outright
    /// configuration error, not a runtime failure.
    fn scheme(&self) -> AuthScheme;

    /// Produce the credential sidecar for `anchor`. The dispatcher
    /// stores this in [`super::log::AnchorLog`] alongside the anchor.
    ///
    /// # Errors
    ///
    /// Returns [`SignerError`] if the signing primitive itself fails
    /// (e.g. malformed key material). Producer-side errors are
    /// distinct from verifier-side errors so callers can distinguish
    /// "I can't produce this anchor" from "I produced a bad anchor".
    fn sign(&self, anchor: &Anchor) -> Result<AnchorAuthCredential, SignerError>;
}

/// Reasons signing can fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignerError {
    /// Underlying signing primitive failed (e.g. RNG, curve op).
    /// `&'static str` for the specific reason — strings appear in
    /// error logs only, not on the wire.
    SignFailed(&'static str),
    /// Anchor's `auth_scheme` doesn't match the signer's `scheme()`.
    /// Caught at sign-time as a safety net; the dispatcher also gates
    /// on this before calling the signer.
    SchemeMismatch,
}

/// ECDSA secp256k1 signer wrapping a `k256` [`SigningKey`]. Produces
/// 65-byte recoverable signatures (r || s || v=27|28) over the
/// EIP-191-prefixed `keccak256(abi.encode(anchor))` digest —
/// `LTPAnchorRegistry.recoverSigner` consumes the same payload shape.
#[derive(Clone)]
pub struct EcdsaSecp256k1Signer {
    inner: SigningKey,
}

impl std::fmt::Debug for EcdsaSecp256k1Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't print the secret key material — only the derived
        // address so logs reveal who's signing, not how.
        f.debug_struct("EcdsaSecp256k1Signer")
            .field("address", &self.address())
            .finish()
    }
}

impl EcdsaSecp256k1Signer {
    /// Wrap a `k256` signing key.
    #[must_use]
    pub fn new(inner: SigningKey) -> Self {
        Self { inner }
    }

    /// The Ethereum address derived from this signer's public key.
    /// `LTPAnchorRegistry.acceptAnchor` checks `recoverSigner == this`
    /// against its `isApprovedSigner` set.
    #[must_use]
    pub fn address(&self) -> EthAddress {
        eth_address_from_verifying_key(self.inner.verifying_key())
    }

    /// Borrow the underlying signing key — for tests that need to
    /// drive the key through other APIs.
    #[doc(hidden)]
    pub fn __inner(&self) -> &SigningKey {
        &self.inner
    }
}

impl AnchorSigner for EcdsaSecp256k1Signer {
    fn scheme(&self) -> AuthScheme {
        AuthScheme::EcdsaSecp256k1
    }

    fn sign(&self, anchor: &Anchor) -> Result<AnchorAuthCredential, SignerError> {
        if anchor.auth_scheme != AuthScheme::EcdsaSecp256k1 {
            return Err(SignerError::SchemeMismatch);
        }
        let digest = eth_signed_message_hash(anchor);
        // PrehashSigner skips the in-library digest pass; we already
        // computed keccak256(EIP-191 || keccak256(abi.encode(anchor))).
        let (sig, recovery): (Signature, RecoveryId) = self
            .inner
            .sign_prehash_recoverable(&digest)
            .map_err(|_| SignerError::SignFailed("k256 sign_prehash_recoverable"))?;

        let mut bytes = [0u8; ECDSA_SIG_LEN];
        let sig_bytes = sig.to_bytes();
        bytes[..32].copy_from_slice(&sig_bytes[..32]);
        bytes[32..64].copy_from_slice(&sig_bytes[32..]);
        // Ethereum's v = recovery_id + 27 (legacy, pre-EIP-155).
        // `LTPAnchorRegistry.recoverSigner` accepts 27 or 28; matching
        // that here keeps the wire bytes identical to a personal_sign
        // call from an EOA wallet.
        bytes[64] = recovery.to_byte() + 27;

        Ok(AnchorAuthCredential::EcdsaSecp256k1 { signature: bytes })
    }
}

/// Derive the 20-byte Ethereum address from a verifying key. Mirrors
/// Solidity's `ecrecover` post-processing: `address(uint160(uint256(
/// keccak256(uncompressed_pubkey[1..]))))`. The leading `0x04` tag is
/// dropped before hashing.
fn eth_address_from_verifying_key(vk: &k256::ecdsa::VerifyingKey) -> EthAddress {
    use sha3::{Digest, Keccak256};

    let point = vk.to_encoded_point(false);
    // First byte is the SEC1 tag (0x04 for uncompressed); skip it.
    let raw = &point.as_bytes()[1..];
    let h = Keccak256::digest(raw);
    let mut out = [0u8; 20];
    out.copy_from_slice(&h[12..32]);
    EthAddress(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::credential::{verify_credential, ExpectedVerifier};
    use super::super::types::{AnchorHash, ChainId, GENESIS_PARENT};
    use suwappudb_state::Commitment;
    use k256::ecdsa::SigningKey;
    use rand::rngs::OsRng;

    fn fresh_signer() -> EcdsaSecp256k1Signer {
        EcdsaSecp256k1Signer::new(SigningKey::random(&mut OsRng))
    }

    fn root(byte: u8) -> Commitment {
        Commitment([byte; 32])
    }

    #[test]
    fn ecdsa_sign_then_verify_credential_round_trips() {
        // S11.3 core: a sign-then-verify cycle accepts the credential
        // under the signer's own address.
        let signer = fresh_signer();
        let anchor = Anchor::ecdsa(ChainId(7), 0, root(1), GENESIS_PARENT);
        let credential = signer.sign(&anchor).expect("sign succeeds");

        verify_credential(
            &anchor,
            &credential,
            &ExpectedVerifier::EcdsaSecp256k1 {
                signer: signer.address(),
            },
        )
        .expect("verify under signer's own address must accept");
    }

    #[test]
    fn ecdsa_credential_rejects_wrong_signer() {
        let signer = fresh_signer();
        let other = fresh_signer();
        let anchor = Anchor::ecdsa(ChainId(7), 0, root(1), GENESIS_PARENT);
        let credential = signer.sign(&anchor).unwrap();

        let res = verify_credential(
            &anchor,
            &credential,
            &ExpectedVerifier::EcdsaSecp256k1 {
                signer: other.address(),
            },
        );
        assert!(res.is_err(), "verify under different signer must reject");
    }

    #[test]
    fn ecdsa_credential_rejects_tampered_state_root() {
        let signer = fresh_signer();
        let anchor = Anchor::ecdsa(ChainId(7), 0, root(1), GENESIS_PARENT);
        let credential = signer.sign(&anchor).unwrap();
        // Tamper post-signing.
        let mut tampered = anchor.clone();
        tampered.state_root = root(99);
        let res = verify_credential(
            &tampered,
            &credential,
            &ExpectedVerifier::EcdsaSecp256k1 {
                signer: signer.address(),
            },
        );
        assert!(res.is_err());
    }

    #[test]
    fn sign_rejects_scheme_mismatch_at_signing_time() {
        let signer = fresh_signer();
        // Build a Blake3-shape anchor (not what the ECDSA signer
        // is configured to sign).
        let blake = Anchor::new(ChainId(1), 0, root(1), GENESIS_PARENT, &[7; 32]);
        let err = signer.sign(&blake).unwrap_err();
        assert_eq!(err, SignerError::SchemeMismatch);
    }

    #[test]
    fn ecdsa_signature_v_byte_is_27_or_28() {
        // LTPAnchorRegistry.recoverSigner enforces v ∈ {27, 28}; our
        // signer must produce those exact values, not the EIP-155
        // chain-mixed variants.
        let signer = fresh_signer();
        let anchor = Anchor::ecdsa(ChainId(7), 0, root(1), GENESIS_PARENT);
        let credential = signer.sign(&anchor).unwrap();
        match credential {
            AnchorAuthCredential::EcdsaSecp256k1 { signature } => {
                assert!(
                    signature[64] == 27 || signature[64] == 28,
                    "v byte was {}, must be 27 or 28",
                    signature[64]
                );
            }
            other => panic!("expected EcdsaSecp256k1 credential, got {other:?}"),
        }
    }

    #[test]
    fn address_is_derived_deterministically() {
        // Same key → same address. Sanity check the keccak->address
        // path.
        let key = SigningKey::from_bytes(&[42u8; 32].into()).unwrap();
        let s1 = EcdsaSecp256k1Signer::new(key.clone());
        let s2 = EcdsaSecp256k1Signer::new(key);
        assert_eq!(s1.address(), s2.address());
    }

    #[test]
    fn debug_does_not_leak_secret_key() {
        let signer = fresh_signer();
        let dbg = format!("{signer:?}");
        // Debug should print address but never the secret key bytes
        // (which would appear as hex if we'd used SigningKey's debug).
        assert!(dbg.contains("EcdsaSecp256k1Signer"));
        assert!(dbg.contains("EthAddress"));
        // The raw scalar field of k256's SigningKey shows up as "scalar"
        // in its Debug — make sure that didn't leak through.
        assert!(!dbg.contains("scalar"));
    }
}
