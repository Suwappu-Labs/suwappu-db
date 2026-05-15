//! End-to-end IQ-7 hybrid-anchor parity tests.
//!
//! Drives the public [`verify_credential`] surface with realistic
//! per-anchor sig sets and asserts the rejection behavior that the
//! Solidity `LTPAnchorRegistry` would also produce.
//!
//! Default build covers `AuthScheme::EcdsaSecp256k1` (the launch-L1
//! path). The `production-pqc` feature additionally drives the
//! `MlDsa65Hybrid` AND-gate end-to-end with both ECDSA and ML-DSA-65
//! signatures over the shared EIP-191 payload.

use gsxdb_bridge::anchor::{
    eth_signed_message_hash, verify_credential, Anchor, AnchorAuthCredential, AnchorHash, ChainId,
    CredentialVerifyError, EcdsaVerifyError, EthAddress, ExpectedVerifier, GENESIS_PARENT,
    ECDSA_SIG_LEN,
};
use gsxdb_bridge::anchor::types::AuthScheme;
use gsxdb_state::Commitment;
use k256::ecdsa::signature::hazmat::PrehashSigner;
use k256::ecdsa::SigningKey;
use rand::rngs::OsRng;

fn ecdsa_anchor() -> Anchor {
    Anchor {
        chain_id: ChainId(103_115_120),
        height: 17,
        state_root: Commitment([0x42; 32]),
        parent: GENESIS_PARENT,
        mac: [0u8; 32],
        auth_scheme: AuthScheme::EcdsaSecp256k1,
    }
}

fn sign(sk: &SigningKey, anchor: &Anchor) -> [u8; ECDSA_SIG_LEN] {
    let payload = eth_signed_message_hash(anchor);
    let (sig, rec) = sk.sign_prehash(&payload).expect("sign");
    let mut bytes = [0u8; ECDSA_SIG_LEN];
    bytes[..64].copy_from_slice(&sig.to_bytes());
    bytes[64] = 27 + rec.to_byte();
    bytes
}

// =========================================================================
// ECDSA-only path (default build, no production-pqc needed)
// =========================================================================

#[test]
fn ecdsa_valid_anchor_accepted_via_dispatch() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let anchor = ecdsa_anchor();
    let sig = sign(&sk, &anchor);

    verify_credential(
        &anchor,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    )
    .expect("valid anchor verifies through dispatch");
}

#[test]
fn ecdsa_rejected_by_unapproved_signer() {
    let sk = SigningKey::random(&mut OsRng);
    let anchor = ecdsa_anchor();
    let sig = sign(&sk, &anchor);

    let result = verify_credential(
        &anchor,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 {
            signer: EthAddress([0xab; 20]),
        },
    );
    assert_eq!(
        result,
        Err(CredentialVerifyError::EcdsaFailed(
            EcdsaVerifyError::UnauthorizedSigner
        ))
    );
}

#[test]
fn ecdsa_rejected_after_state_root_tamper() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let original = ecdsa_anchor();
    let sig = sign(&sk, &original);

    let mut tampered = original;
    tampered.state_root = Commitment([0xff; 32]);

    let result = verify_credential(
        &tampered,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    // Either recovers a different address (UnauthorizedSigner) or
    // recovery fails outright — both are rejections.
    assert!(matches!(
        result,
        Err(CredentialVerifyError::EcdsaFailed(
            EcdsaVerifyError::UnauthorizedSigner | EcdsaVerifyError::RecoveryFailed
        ))
    ));
}

#[test]
fn ecdsa_rejected_after_height_tamper() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let original = ecdsa_anchor();
    let sig = sign(&sk, &original);

    let mut tampered = original;
    tampered.height += 1;

    let result = verify_credential(
        &tampered,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    assert!(matches!(result, Err(CredentialVerifyError::EcdsaFailed(_))));
}

#[test]
fn ecdsa_rejected_after_chain_id_tamper() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let original = ecdsa_anchor();
    let sig = sign(&sk, &original);

    let mut tampered = original;
    tampered.chain_id = ChainId(99);

    let result = verify_credential(
        &tampered,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    assert!(matches!(result, Err(CredentialVerifyError::EcdsaFailed(_))));
}

#[test]
fn ecdsa_rejected_after_parent_tamper() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let original = ecdsa_anchor();
    let sig = sign(&sk, &original);

    let mut tampered = original;
    tampered.parent = AnchorHash([0xde; 32]);

    let result = verify_credential(
        &tampered,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    assert!(matches!(result, Err(CredentialVerifyError::EcdsaFailed(_))));
}

#[test]
fn ecdsa_rejected_after_mac_field_tamper() {
    // The Solidity abi.encode covers the mac field too, so mutating it
    // after signing invalidates the ECDSA payload.
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let original = ecdsa_anchor();
    let sig = sign(&sk, &original);

    let mut tampered = original;
    tampered.mac = [0xaa; 32];

    let result = verify_credential(
        &tampered,
        &AnchorAuthCredential::EcdsaSecp256k1 { signature: sig },
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    assert!(matches!(result, Err(CredentialVerifyError::EcdsaFailed(_))));
}

#[test]
fn scheme_mismatch_ecdsa_declared_blake3_supplied() {
    let sk = SigningKey::random(&mut OsRng);
    let signer = EthAddress::from_verifying_key(sk.verifying_key());
    let anchor = ecdsa_anchor();

    // Caller mistakenly passes a Blake3Mac credential for an ECDSA anchor.
    let result = verify_credential(
        &anchor,
        &AnchorAuthCredential::Blake3Mac,
        &ExpectedVerifier::EcdsaSecp256k1 { signer },
    );
    assert_eq!(result, Err(CredentialVerifyError::SchemeMismatch));
}

#[test]
fn scheme_mismatch_blake3_anchor_ecdsa_credential() {
    let key = [3u8; 32];
    let blake_anchor = Anchor::new(ChainId(1), 0, Commitment([1; 32]), GENESIS_PARENT, &key);

    let result = verify_credential(
        &blake_anchor,
        &AnchorAuthCredential::EcdsaSecp256k1 {
            signature: [0; ECDSA_SIG_LEN],
        },
        &ExpectedVerifier::Blake3Mac { key: &key },
    );
    assert_eq!(result, Err(CredentialVerifyError::SchemeMismatch));
}

// =========================================================================
// Hybrid AND-gate path (production-pqc feature)
// =========================================================================

#[cfg(feature = "production-pqc")]
mod hybrid {
    use super::*;
    use gsxdb_bridge::anchor::MlDsaVerifyError;
    use pqcrypto_mldsa::mldsa65;
    use pqcrypto_traits::sign::{DetachedSignature as _, PublicKey as _};

    struct HybridFixture {
        anchor: Anchor,
        ecdsa_signer: EthAddress,
        ecdsa_signature: [u8; ECDSA_SIG_LEN],
        mldsa_public_key: Vec<u8>,
        mldsa_signature: Vec<u8>,
    }

    fn fresh() -> HybridFixture {
        let sk = SigningKey::random(&mut OsRng);
        let signer = EthAddress::from_verifying_key(sk.verifying_key());

        let mut anchor = ecdsa_anchor();
        anchor.auth_scheme = AuthScheme::MlDsa65Hybrid;

        let ecdsa_signature = sign(&sk, &anchor);

        let (pk, sk_pq) = mldsa65::keypair();
        let payload = eth_signed_message_hash(&anchor);
        let pq_sig = mldsa65::detached_sign(&payload, &sk_pq);

        HybridFixture {
            anchor,
            ecdsa_signer: signer,
            ecdsa_signature,
            mldsa_public_key: pk.as_bytes().to_vec(),
            mldsa_signature: pq_sig.as_bytes().to_vec(),
        }
    }

    #[test]
    fn hybrid_accepts_when_both_halves_valid() {
        let f = fresh();
        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: f.ecdsa_signature,
            mldsa_signature: f.mldsa_signature,
        };
        verify_credential(
            &f.anchor,
            &cred,
            &ExpectedVerifier::MlDsa65Hybrid {
                signer: f.ecdsa_signer,
                mldsa_public_key: &f.mldsa_public_key,
            },
        )
        .expect("hybrid accepts when both halves verify");
    }

    #[test]
    fn hybrid_rejects_when_ecdsa_signer_unauthorized() {
        let f = fresh();
        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: f.ecdsa_signature,
            mldsa_signature: f.mldsa_signature,
        };
        let result = verify_credential(
            &f.anchor,
            &cred,
            &ExpectedVerifier::MlDsa65Hybrid {
                signer: EthAddress([0xff; 20]),
                mldsa_public_key: &f.mldsa_public_key,
            },
        );
        assert!(matches!(
            result,
            Err(CredentialVerifyError::EcdsaFailed(
                EcdsaVerifyError::UnauthorizedSigner
            ))
        ));
    }

    #[test]
    fn hybrid_rejects_when_mldsa_public_key_wrong() {
        let f = fresh();
        let (other_pk, _) = mldsa65::keypair();
        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: f.ecdsa_signature,
            mldsa_signature: f.mldsa_signature,
        };
        let result = verify_credential(
            &f.anchor,
            &cred,
            &ExpectedVerifier::MlDsa65Hybrid {
                signer: f.ecdsa_signer,
                mldsa_public_key: other_pk.as_bytes(),
            },
        );
        assert!(matches!(
            result,
            Err(CredentialVerifyError::MlDsaFailed(
                MlDsaVerifyError::InvalidSignature
            ))
        ));
    }

    #[test]
    fn hybrid_short_circuits_to_ecdsa_failure_when_both_invalid() {
        // Both halves are wrong; ECDSA is checked first, so the
        // error must name the ECDSA failure (not ML-DSA).
        let f = fresh();
        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: f.ecdsa_signature,
            mldsa_signature: vec![0u8; 64], // garbage PQ sig
        };
        let result = verify_credential(
            &f.anchor,
            &cred,
            &ExpectedVerifier::MlDsa65Hybrid {
                signer: EthAddress([0xff; 20]),
                mldsa_public_key: &f.mldsa_public_key,
            },
        );
        assert!(matches!(
            result,
            Err(CredentialVerifyError::EcdsaFailed(_))
        ));
    }

    #[test]
    fn hybrid_rejects_when_payload_tampered_after_sign() {
        let mut f = fresh();
        f.anchor.state_root = Commitment([0xee; 32]);

        let cred = AnchorAuthCredential::MlDsa65Hybrid {
            ecdsa_signature: f.ecdsa_signature,
            mldsa_signature: f.mldsa_signature,
        };
        let result = verify_credential(
            &f.anchor,
            &cred,
            &ExpectedVerifier::MlDsa65Hybrid {
                signer: f.ecdsa_signer,
                mldsa_public_key: &f.mldsa_public_key,
            },
        );
        // ECDSA short-circuits first: tampered payload recovers a
        // different address or fails recovery.
        assert!(matches!(
            result,
            Err(CredentialVerifyError::EcdsaFailed(_))
        ));
    }
}
