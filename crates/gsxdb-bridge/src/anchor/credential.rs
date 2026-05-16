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

use super::types::{Anchor, AnchorHash, AuthScheme, ChainId};
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

/// Public values committed to by an Sp1ZkProof guest program over a
/// single-block replay.
///
/// The guest body wraps [`crate::recovery::replay`]:
///
/// ```text
/// input:  (prev_state_root, block)
/// output: (prev_state_root, new_state_root, block_hash)
/// ```
///
/// The `block_hash` is committed alongside the roots so the verifier
/// can bind the proof to a specific block, not just to any block that
/// happens to produce the claimed new root.
///
/// Encoding is fixed-byte (3 × 32 = 96 bytes) so the eventual zk
/// circuit can commit to it without ABI machinery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sp1PublicValues {
    /// State root the block consumed.
    pub prev_state_root: [u8; 32],
    /// State root the block produced.
    pub new_state_root: [u8; 32],
    /// Hash of the block whose replay was proven.
    pub block_hash: [u8; 32],
}

impl Sp1PublicValues {
    /// Canonical 96-byte encoding the zkVM commits to.
    #[must_use]
    pub fn to_bytes(&self) -> [u8; 96] {
        let mut out = [0u8; 96];
        out[0..32].copy_from_slice(&self.prev_state_root);
        out[32..64].copy_from_slice(&self.new_state_root);
        out[64..96].copy_from_slice(&self.block_hash);
        out
    }
}

/// Sidecar credential carrying real signature bytes alongside an
/// [`Anchor`]. Variants are 1:1 with
/// [`AuthScheme`](super::types::AuthScheme).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorAuthCredential {
    /// `Blake3Mac` carries its MAC inline in [`Anchor::mac`] — no
    /// sidecar bytes needed. This variant exists so dispatch is
    /// total over all schemes.
    Blake3Mac,
    /// `Sp1ZkProof` validity-proof bundle. The guest program (replay
    /// of a single block) commits to [`Sp1PublicValues`] and produces
    /// the opaque proof bytes. The verifier (pending Track 1.3 Step 2)
    /// will check that:
    ///   1. `vkey_hash` matches the registered guest-program hash;
    ///   2. the proof verifies under `vkey_hash` and `public_values`;
    ///   3. `public_values` agrees with the anchor's stated transition
    ///      and the chain's last accepted state root.
    ///
    /// Today the wire shape is fixed but cryptographic verification
    /// is not wired (zkVM toolchain choice is open).
    Sp1ZkProof {
        /// Hash of the verifying-key for the zk program that produced
        /// `proof`. Pinning the vkey hash binds the proof to a specific
        /// compiled guest binary; mismatch is unconditional rejection.
        vkey_hash: [u8; 32],
        /// The values the guest program committed to. Carried in the
        /// clear so the verifier can cross-check them against the
        /// anchor's claimed transition before doing the heavy proof
        /// check.
        public_values: Sp1PublicValues,
        /// Opaque proof bytes produced by the zkVM. Variable length;
        /// schema is owned by the prover crate (sp1-sdk / risc0 / noir
        /// — pending Track 1.3 Step 2 decision).
        proof_bytes: Vec<u8>,
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
/// Per-scheme verifier inputs supplied by the caller of
/// [`verify_credential`].
///
/// Variants must match the [`AuthScheme`] of the anchor being verified
/// and the variant of [`AnchorAuthCredential`] carrying the sig bytes.
/// Mismatch surfaces as [`CredentialVerifyError::SchemeMismatch`].
#[derive(Debug, Clone, Copy)]
pub enum ExpectedVerifier<'a> {
    /// Symmetric BLAKE3 keyed-MAC. The `key` is the same per-chain key
    /// the dispatcher uses for `Anchor::new`.
    Blake3Mac {
        /// Per-chain symmetric authenticator key.
        key: &'a [u8; 32],
    },
    /// secp256k1 approved signer (recovered Ethereum address).
    EcdsaSecp256k1 {
        /// Address authorized to sign anchors for this chain.
        signer: EthAddress,
    },
    /// Hybrid: an approved ECDSA signer AND an ML-DSA-65 public key.
    /// Both must verify for the anchor to be accepted.
    MlDsa65Hybrid {
        /// ECDSA half: approved Ethereum signer.
        signer: EthAddress,
        /// ML-DSA-65 half: raw public key bytes (FIPS 204 encoding).
        mldsa_public_key: &'a [u8],
    },
    /// Sp1 (or other zkVM) validity-proof: the verifier checks that
    /// (a) the proof was produced by the program whose verifying-key
    /// hashes to `vkey_hash`, and (b) the committed public values
    /// describe the transition the anchor claims.
    Sp1ZkProof {
        /// Hash of the registered guest-program verifying key. The
        /// credential's `vkey_hash` field must match this exactly.
        vkey_hash: [u8; 32],
        /// Expected `prev_state_root` — typically the previously
        /// accepted anchor's `state_root` for this chain.
        expected_prev_state_root: [u8; 32],
        /// Expected `new_state_root` — equal to the anchor's
        /// `state_root` field.
        expected_new_state_root: [u8; 32],
        /// Expected `block_hash` — the hash of the block whose replay
        /// the proof attests to.
        expected_block_hash: [u8; 32],
    },
}

/// Owned per-chain verifier configuration. Carried by
/// [`crate::anchor::dispatcher::AnchorDispatcher`]; converts to
/// [`ExpectedVerifier`] on demand for [`verify_credential`] dispatch.
///
/// Variants mirror [`ExpectedVerifier`] but own their bytes instead of
/// borrowing them — the dispatcher stores one config per chain in a
/// `BTreeMap`, so the borrow form isn't usable directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifierConfig {
    /// Symmetric BLAKE3 keyed-MAC. Legacy path — used by all chains
    /// before S11.
    Blake3Mac {
        /// Per-chain symmetric authenticator key.
        key: [u8; 32],
    },
    /// secp256k1 approved signer. Anchors are accepted iff the ECDSA
    /// recoverable signature recovers to `signer`.
    EcdsaSecp256k1 {
        /// Authorized Ethereum signer address (20 bytes).
        signer: EthAddress,
    },
    /// Hybrid ECDSA + ML-DSA-65 AND-gate. Both halves must verify.
    /// Only used when the bridge is built with `production-pqc`; the
    /// runtime check rejects the scheme on non-PQC builds.
    MlDsa65Hybrid {
        /// ECDSA half.
        signer: EthAddress,
        /// ML-DSA-65 raw public-key bytes (FIPS 204 encoding).
        mldsa_public_key: Vec<u8>,
    },
    /// Sp1 (or other zkVM) validity-proof — for anchors that carry
    /// a zk proof of correct block-replay.
    Sp1ZkProof {
        /// Hash of the registered guest-program verifying key.
        vkey_hash: [u8; 32],
        /// Last accepted `state_root` for this chain. Updated by the
        /// dispatcher as anchors are appended.
        expected_prev_state_root: [u8; 32],
        /// `block_hash` the next anchor's proof must commit to.
        /// Updated alongside `expected_prev_state_root`.
        expected_block_hash: [u8; 32],
    },
}

impl VerifierConfig {
    /// Borrow this config as an [`ExpectedVerifier`] for a specific
    /// anchor. The anchor's `state_root` populates the
    /// `expected_new_state_root` field for the Sp1 variant.
    #[must_use]
    pub fn as_expected_for<'a>(&'a self, anchor: &'a Anchor) -> ExpectedVerifier<'a> {
        match self {
            VerifierConfig::Blake3Mac { key } => ExpectedVerifier::Blake3Mac { key },
            VerifierConfig::EcdsaSecp256k1 { signer } => {
                ExpectedVerifier::EcdsaSecp256k1 { signer: *signer }
            }
            VerifierConfig::MlDsa65Hybrid {
                signer,
                mldsa_public_key,
            } => ExpectedVerifier::MlDsa65Hybrid {
                signer: *signer,
                mldsa_public_key: mldsa_public_key.as_slice(),
            },
            VerifierConfig::Sp1ZkProof {
                vkey_hash,
                expected_prev_state_root,
                expected_block_hash,
            } => ExpectedVerifier::Sp1ZkProof {
                vkey_hash: *vkey_hash,
                expected_prev_state_root: *expected_prev_state_root,
                expected_new_state_root: anchor.state_root.0,
                expected_block_hash: *expected_block_hash,
            },
        }
    }

    /// Which [`AuthScheme`] this config will accept on incoming
    /// anchors. The dispatcher uses this to choose the
    /// [`AnchorAuthCredential`] variant when *producing* anchors.
    #[must_use]
    pub fn scheme(&self) -> AuthScheme {
        match self {
            VerifierConfig::Blake3Mac { .. } => AuthScheme::Blake3Mac,
            VerifierConfig::EcdsaSecp256k1 { .. } => AuthScheme::EcdsaSecp256k1,
            VerifierConfig::MlDsa65Hybrid { .. } => AuthScheme::MlDsa65Hybrid,
            VerifierConfig::Sp1ZkProof { .. } => AuthScheme::Sp1ZkProof,
        }
    }
}

/// Reasons [`verify_credential`] can fail. Distinct from the per-scheme
/// `*VerifyError` types so the AND-gate caller can see which half of a
/// hybrid failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialVerifyError {
    /// `anchor.auth_scheme`, the credential variant, and the
    /// `ExpectedVerifier` variant did not all agree.
    SchemeMismatch,
    /// The scheme has no verifier wired in this build (e.g.
    /// `MlDsa65Hybrid` without the `production-pqc` feature, or
    /// `Sp1ZkProof` until Track 1.3 lands).
    UnsupportedScheme(AuthScheme),
    /// `Blake3Mac` MAC did not verify under the supplied key.
    Blake3MacInvalid,
    /// ECDSA half failed (also raised for the ECDSA-only scheme).
    EcdsaFailed(EcdsaVerifyError),
    /// ML-DSA-65 half failed. Only emitted with `production-pqc`.
    #[cfg(feature = "production-pqc")]
    MlDsaFailed(MlDsaVerifyError),
    /// Sp1 credential's `vkey_hash` did not match the registered
    /// verifying-key hash. Emitted before the proof-verify step.
    Sp1VkeyMismatch,
    /// Sp1 credential's `public_values` did not agree with the
    /// expected transition for this anchor. Emitted before the
    /// proof-verify step.
    Sp1PublicValuesMismatch,
}

/// Top-level verifier: dispatches on `anchor.auth_scheme` and runs the
/// per-scheme check. For `MlDsa65Hybrid` this is the AND-gate — both
/// ECDSA and ML-DSA-65 must verify, with the *same* underlying payload.
///
/// # Errors
///
/// Returns [`CredentialVerifyError`] with the specific failure reason.
/// For hybrid schemes, the first failing component short-circuits the
/// other so the caller knows exactly which half rejected.
pub fn verify_credential(
    anchor: &Anchor,
    credential: &AnchorAuthCredential,
    expected: &ExpectedVerifier<'_>,
) -> Result<(), CredentialVerifyError> {
    match (anchor.auth_scheme, credential, expected) {
        (
            AuthScheme::Blake3Mac,
            AnchorAuthCredential::Blake3Mac,
            ExpectedVerifier::Blake3Mac { key },
        ) => {
            if anchor.verify_mac(key) {
                Ok(())
            } else {
                Err(CredentialVerifyError::Blake3MacInvalid)
            }
        }
        (
            AuthScheme::EcdsaSecp256k1,
            AnchorAuthCredential::EcdsaSecp256k1 { signature },
            ExpectedVerifier::EcdsaSecp256k1 { signer },
        ) => verify_ecdsa(anchor, signature, signer).map_err(CredentialVerifyError::EcdsaFailed),
        (
            AuthScheme::MlDsa65Hybrid,
            AnchorAuthCredential::MlDsa65Hybrid {
                ecdsa_signature,
                mldsa_signature,
            },
            ExpectedVerifier::MlDsa65Hybrid {
                signer,
                mldsa_public_key,
            },
        ) => verify_hybrid(
            anchor,
            ecdsa_signature,
            mldsa_signature,
            signer,
            mldsa_public_key,
        ),
        (
            AuthScheme::Sp1ZkProof,
            AnchorAuthCredential::Sp1ZkProof {
                vkey_hash,
                public_values,
                proof_bytes,
            },
            ExpectedVerifier::Sp1ZkProof {
                vkey_hash: expected_vkey,
                expected_prev_state_root,
                expected_new_state_root,
                expected_block_hash,
            },
        ) => {
            // Cheap structural pre-checks the future cryptographic
            // verifier would also do — surfaced here so callers can
            // tell vkey-mismatch from public-values-mismatch from
            // proof-failure.
            if vkey_hash != expected_vkey {
                return Err(CredentialVerifyError::Sp1VkeyMismatch);
            }
            if &public_values.prev_state_root != expected_prev_state_root
                || &public_values.new_state_root != expected_new_state_root
                || &public_values.block_hash != expected_block_hash
            {
                return Err(CredentialVerifyError::Sp1PublicValuesMismatch);
            }
            // Cryptographic proof verification is not wired yet —
            // pending Track 1.3 Step 2 (zkVM toolchain decision +
            // sp1-sdk / risc0 / noir integration). Until then, the
            // arm is conservative: structurally valid bundles are
            // STILL rejected as unsupported so no caller mistakes
            // pre-check success for full verification.
            let _ = proof_bytes;
            Err(CredentialVerifyError::UnsupportedScheme(
                AuthScheme::Sp1ZkProof,
            ))
        }
        (AuthScheme::Sp1ZkProof, _, _) => Err(CredentialVerifyError::SchemeMismatch),
        _ => Err(CredentialVerifyError::SchemeMismatch),
    }
}

/// AND-gate composer for the `MlDsa65Hybrid` scheme. Verifies the
/// ECDSA half first (cheaper) then the ML-DSA-65 half. Either failure
/// rejects the anchor.
#[cfg(feature = "production-pqc")]
fn verify_hybrid(
    anchor: &Anchor,
    ecdsa_signature: &[u8; ECDSA_SIG_LEN],
    mldsa_signature: &[u8],
    signer: &EthAddress,
    mldsa_public_key: &[u8],
) -> Result<(), CredentialVerifyError> {
    verify_ecdsa(anchor, ecdsa_signature, signer).map_err(CredentialVerifyError::EcdsaFailed)?;
    verify_mldsa65(anchor, mldsa_signature, mldsa_public_key)
        .map_err(CredentialVerifyError::MlDsaFailed)?;
    Ok(())
}

/// Stub used when the `production-pqc` feature is off — surfaces
/// `UnsupportedScheme` rather than silently ignoring the hybrid claim.
#[cfg(not(feature = "production-pqc"))]
#[allow(clippy::needless_pass_by_value)]
fn verify_hybrid(
    _anchor: &Anchor,
    _ecdsa_signature: &[u8; ECDSA_SIG_LEN],
    _mldsa_signature: &[u8],
    _signer: &EthAddress,
    _mldsa_public_key: &[u8],
) -> Result<(), CredentialVerifyError> {
    Err(CredentialVerifyError::UnsupportedScheme(
        AuthScheme::MlDsa65Hybrid,
    ))
}

/// Verify an ECDSA secp256k1 signature against the EIP-191 anchor
/// payload and check the recovered Ethereum address matches
/// `expected_signer`. Mirrors `LTPAnchorRegistry.recoverSigner` +
/// `isApprovedSigner[signer]`.
///
/// # Errors
///
/// Returns [`EcdsaVerifyError`] when the signature is malformed,
/// recovery fails, or the recovered address is not the expected signer.
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
    // EIP-2 malleability: reject high-s. Mirrors Solidity LTPAnchorRegistry.
    if sig.normalize_s().is_some() {
        return Err(EcdsaVerifyError::MalformedSignature);
    }

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
    fn ecdsa_rejects_high_s_malleable_signature() {
        // EIP-2: a valid (r, s) signature has a malleable twin (r, n-s).
        // Solidity's LTPAnchorRegistry rejects high-s; we mirror that.
        let sk = SigningKey::random(&mut OsRng);
        let signer = EthAddress::from_verifying_key(sk.verifying_key());

        let anchor = sample_anchor();
        let sig = sign_anchor(&sk, &anchor);

        // Flip s -> n - s by negating the s scalar.
        let low_s = Signature::try_from(&sig[..64]).expect("valid sig");
        let s_neg = -*low_s.s();
        let high_s = Signature::from_scalars(low_s.r().to_bytes(), s_neg.to_bytes())
            .expect("non-zero scalars");
        let mut malleable = [0u8; ECDSA_SIG_LEN];
        malleable[..64].copy_from_slice(&high_s.to_bytes());
        malleable[64] = sig[64];

        assert_eq!(
            verify_ecdsa(&anchor, &malleable, &signer),
            Err(EcdsaVerifyError::MalformedSignature)
        );
    }

    fn blake3_anchor(key: &[u8; 32]) -> Anchor {
        Anchor::new(
            ChainId(1),
            7,
            Commitment([0x11; 32]),
            GENESIS_PARENT,
            key,
        )
    }

    #[test]
    fn verify_credential_blake3_roundtrip() {
        let key = [9u8; 32];
        let anchor = blake3_anchor(&key);
        verify_credential(
            &anchor,
            &AnchorAuthCredential::Blake3Mac,
            &ExpectedVerifier::Blake3Mac { key: &key },
        )
        .expect("blake3 path verifies");
    }

    #[test]
    fn verify_credential_blake3_rejects_bad_key() {
        let key = [9u8; 32];
        let anchor = blake3_anchor(&key);
        let wrong = [0u8; 32];
        assert_eq!(
            verify_credential(
                &anchor,
                &AnchorAuthCredential::Blake3Mac,
                &ExpectedVerifier::Blake3Mac { key: &wrong },
            ),
            Err(CredentialVerifyError::Blake3MacInvalid)
        );
    }

    #[test]
    fn verify_credential_scheme_mismatch() {
        let key = [9u8; 32];
        let anchor = blake3_anchor(&key);
        // Anchor declares Blake3Mac but caller supplies an ECDSA credential.
        assert_eq!(
            verify_credential(
                &anchor,
                &AnchorAuthCredential::EcdsaSecp256k1 {
                    signature: [0u8; ECDSA_SIG_LEN]
                },
                &ExpectedVerifier::Blake3Mac { key: &key },
            ),
            Err(CredentialVerifyError::SchemeMismatch)
        );
    }

    #[test]
    fn verify_credential_ecdsa_via_dispatch() {
        let sk = SigningKey::random(&mut OsRng);
        let signer = EthAddress::from_verifying_key(sk.verifying_key());
        let anchor = sample_anchor(); // already AuthScheme::EcdsaSecp256k1
        let sig = sign_anchor(&sk, &anchor);

        verify_credential(
            &anchor,
            &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
            &ExpectedVerifier::EcdsaSecp256k1 { signer },
        )
        .expect("ecdsa path verifies via dispatch");
    }

    fn sp1_credential(
        vkey_hash: [u8; 32],
        prev: [u8; 32],
        new: [u8; 32],
        block_hash: [u8; 32],
        proof_bytes: Vec<u8>,
    ) -> AnchorAuthCredential {
        AnchorAuthCredential::Sp1ZkProof {
            vkey_hash,
            public_values: Sp1PublicValues {
                prev_state_root: prev,
                new_state_root: new,
                block_hash,
            },
            proof_bytes,
        }
    }

    fn sp1_expected(
        vkey_hash: [u8; 32],
        prev: [u8; 32],
        new: [u8; 32],
        block_hash: [u8; 32],
    ) -> ExpectedVerifier<'static> {
        ExpectedVerifier::Sp1ZkProof {
            vkey_hash,
            expected_prev_state_root: prev,
            expected_new_state_root: new,
            expected_block_hash: block_hash,
        }
    }

    #[test]
    fn sp1_public_values_encoding_is_96_bytes() {
        let pv = Sp1PublicValues {
            prev_state_root: [0xaa; 32],
            new_state_root: [0xbb; 32],
            block_hash: [0xcc; 32],
        };
        let bytes = pv.to_bytes();
        assert_eq!(bytes.len(), 96);
        assert_eq!(&bytes[0..32], &[0xaa; 32]);
        assert_eq!(&bytes[32..64], &[0xbb; 32]);
        assert_eq!(&bytes[64..96], &[0xcc; 32]);
    }

    #[test]
    fn verify_credential_sp1_scheme_mismatch_when_credential_is_blake3() {
        let mut anchor = sample_anchor();
        anchor.auth_scheme = AuthScheme::Sp1ZkProof;
        // Caller passes an ECDSA expected-verifier — that's a hard
        // scheme mismatch regardless of the credential's value match.
        let result = verify_credential(
            &anchor,
            &AnchorAuthCredential::Blake3Mac,
            &ExpectedVerifier::EcdsaSecp256k1 {
                signer: EthAddress([0; 20]),
            },
        );
        assert_eq!(result, Err(CredentialVerifyError::SchemeMismatch));
    }

    #[test]
    fn verify_credential_sp1_rejects_vkey_mismatch() {
        let mut anchor = sample_anchor();
        anchor.auth_scheme = AuthScheme::Sp1ZkProof;
        let cred = sp1_credential([1; 32], [2; 32], [3; 32], [4; 32], vec![0xab; 16]);
        // Expected vkey differs:
        let expected = sp1_expected([9; 32], [2; 32], [3; 32], [4; 32]);
        assert_eq!(
            verify_credential(&anchor, &cred, &expected),
            Err(CredentialVerifyError::Sp1VkeyMismatch)
        );
    }

    #[test]
    fn verify_credential_sp1_rejects_public_values_mismatch() {
        let mut anchor = sample_anchor();
        anchor.auth_scheme = AuthScheme::Sp1ZkProof;
        let cred = sp1_credential([1; 32], [2; 32], [3; 32], [4; 32], vec![0xab; 16]);
        // Same vkey, different new_state_root:
        let expected = sp1_expected([1; 32], [2; 32], [0xff; 32], [4; 32]);
        assert_eq!(
            verify_credential(&anchor, &cred, &expected),
            Err(CredentialVerifyError::Sp1PublicValuesMismatch)
        );
    }

    #[test]
    fn verify_credential_sp1_returns_unsupported_when_prechecks_pass() {
        // Structurally valid: vkey matches, public values match. The
        // arm STILL rejects with UnsupportedScheme because the proof
        // verifier isn't wired. This is the load-bearing guarantee:
        // pre-check success is never confused for full verification.
        let mut anchor = sample_anchor();
        anchor.auth_scheme = AuthScheme::Sp1ZkProof;
        let cred = sp1_credential([1; 32], [2; 32], [3; 32], [4; 32], vec![0xab; 16]);
        let expected = sp1_expected([1; 32], [2; 32], [3; 32], [4; 32]);
        assert_eq!(
            verify_credential(&anchor, &cred, &expected),
            Err(CredentialVerifyError::UnsupportedScheme(
                AuthScheme::Sp1ZkProof
            ))
        );
    }

    #[cfg(not(feature = "production-pqc"))]
    #[test]
    fn verify_credential_hybrid_returns_unsupported_without_feature() {
        let mut anchor = sample_anchor();
        anchor.auth_scheme = AuthScheme::MlDsa65Hybrid;
        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: [0u8; ECDSA_SIG_LEN],
            mldsa_signature: vec![],
        };
        let expected = ExpectedVerifier::MlDsa65Hybrid {
            signer: EthAddress([0; 20]),
            mldsa_public_key: &[],
        };
        assert_eq!(
            verify_credential(&anchor, &cred, &expected),
            Err(CredentialVerifyError::UnsupportedScheme(
                AuthScheme::MlDsa65Hybrid
            ))
        );
    }

    #[cfg(feature = "production-pqc")]
    mod hybrid {
        use super::super::*;
        use super::{sample_anchor, sign_anchor};
        use k256::ecdsa::SigningKey;
        use pqcrypto_mldsa::mldsa65;
        use pqcrypto_traits::sign::{DetachedSignature as _, PublicKey as _};
        use rand::rngs::OsRng;

        fn setup() -> (
            Anchor,
            EthAddress,
            [u8; ECDSA_SIG_LEN],
            Vec<u8>,   // pq sig
            Vec<u8>,   // pq pubkey
            EthAddress,  // wrong_signer
            Vec<u8>,   // wrong_pq_pubkey
        ) {
            let sk = SigningKey::random(&mut OsRng);
            let signer = EthAddress::from_verifying_key(sk.verifying_key());
            let mut anchor = sample_anchor();
            anchor.auth_scheme = AuthScheme::MlDsa65Hybrid;
            let ecdsa_sig = sign_anchor(&sk, &anchor);

            let (pq_pk, pq_sk) = mldsa65::keypair();
            let payload = eth_signed_message_hash(&anchor);
            let pq_sig = mldsa65::detached_sign(&payload, &pq_sk);

            let (other_pk, _) = mldsa65::keypair();

            (
                anchor,
                signer,
                ecdsa_sig,
                pq_sig.as_bytes().to_vec(),
                pq_pk.as_bytes().to_vec(),
                EthAddress([0xee; 20]),
                other_pk.as_bytes().to_vec(),
            )
        }

        #[test]
        fn hybrid_and_gate_accepts_when_both_verify() {
            let (anchor, signer, ecdsa_sig, pq_sig, pq_pk, _, _) = setup();
            let cred = AnchorAuthCredential::MlDsa65Hybrid {
                ecdsa_signature: ecdsa_sig,
                mldsa_signature: pq_sig,
            };
            let expected = ExpectedVerifier::MlDsa65Hybrid {
                signer,
                mldsa_public_key: &pq_pk,
            };
            verify_credential(&anchor, &cred, &expected).expect("both halves verify");
        }

        #[test]
        fn hybrid_and_gate_rejects_when_ecdsa_fails() {
            let (anchor, _, ecdsa_sig, pq_sig, pq_pk, wrong_signer, _) = setup();
            let cred = AnchorAuthCredential::MlDsa65Hybrid {
                ecdsa_signature: ecdsa_sig,
                mldsa_signature: pq_sig,
            };
            let expected = ExpectedVerifier::MlDsa65Hybrid {
                signer: wrong_signer,
                mldsa_public_key: &pq_pk,
            };
            assert!(matches!(
                verify_credential(&anchor, &cred, &expected),
                Err(CredentialVerifyError::EcdsaFailed(_))
            ));
        }

        #[test]
        fn hybrid_and_gate_rejects_when_mldsa_fails() {
            let (anchor, signer, ecdsa_sig, pq_sig, _, _, wrong_pq_pk) = setup();
            let cred = AnchorAuthCredential::MlDsa65Hybrid {
                ecdsa_signature: ecdsa_sig,
                mldsa_signature: pq_sig,
            };
            let expected = ExpectedVerifier::MlDsa65Hybrid {
                signer,
                mldsa_public_key: &wrong_pq_pk,
            };
            assert!(matches!(
                verify_credential(&anchor, &cred, &expected),
                Err(CredentialVerifyError::MlDsaFailed(_))
            ));
        }

        #[test]
        fn hybrid_and_gate_short_circuits_on_ecdsa_failure() {
            // Use deliberately invalid pq_sig: if ECDSA short-circuits
            // first, we should get EcdsaFailed, not MlDsaFailed.
            let (anchor, _, ecdsa_sig, _, pq_pk, wrong_signer, _) = setup();
            let cred = AnchorAuthCredential::MlDsa65Hybrid {
                ecdsa_signature: ecdsa_sig,
                mldsa_signature: vec![0u8; 16], // garbage
            };
            let expected = ExpectedVerifier::MlDsa65Hybrid {
                signer: wrong_signer,
                mldsa_public_key: &pq_pk,
            };
            assert!(matches!(
                verify_credential(&anchor, &cred, &expected),
                Err(CredentialVerifyError::EcdsaFailed(_))
            ));
        }
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
