//! **S11.5 cross-impl parity vector generator.**
//!
//! Produces deterministic ECDSA-signed anchor vectors that the
//! Foundry-side differential test
//! (`contracts/test/LTPAnchorRegistryParity.t.sol`) consumes via
//! `vm.parseJson`. Each vector contains:
//!
//! - The anchor (chainId, height, stateRoot, parent, mac)
//! - The 65-byte ECDSA signature produced by `EcdsaSecp256k1Signer`
//! - The expected recovered signer address
//!
//! The Foundry test calls `recoverSigner(anchor, signature)` and
//! asserts the result equals the embedded signer address — proving
//! the Rust signing pipeline and the Solidity verifier produce
//! bit-identical EIP-191 payloads.
//!
//! Usage:
//!
//! ```text
//!   cargo run --example gen_parity_vectors --release \
//!       -- contracts/test/fixtures/parity_vectors.json
//! ```
//!
//! Deterministic: seeds the signing key from a fixed byte array so CI
//! runs are reproducible. Run with `RANDOM_SEED=1` (any non-zero) to
//! randomize for hand-checks.

use suwappudb_bridge::anchor::{
    AnchorAuthCredential, AnchorSigner, EcdsaSecp256k1Signer,
};
use suwappudb_bridge::{Anchor, ChainId, GENESIS_PARENT};
use suwappudb_state::Commitment;
use k256::ecdsa::SigningKey;
use serde_json::json;
use std::env;
use std::fs;
use std::path::PathBuf;

const NUM_VECTORS: usize = 16;

fn hex_bytes(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + bytes.len() * 2);
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let out_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "parity_vectors.json".to_string());
    let out_path = PathBuf::from(out_path);

    // Deterministic 32-byte seed. Don't randomize unless asked — CI
    // wants stable artifacts.
    let seed = if env::var("RANDOM_SEED").is_ok() {
        let mut buf = [0u8; 32];
        use rand::RngCore;
        rand::rngs::OsRng.fill_bytes(&mut buf);
        buf
    } else {
        [0x42u8; 32]
    };
    let key = SigningKey::from_bytes(&seed.into()).expect("valid 32-byte scalar");
    let signer = EcdsaSecp256k1Signer::new(key);
    let signer_addr = signer.address();

    let mut vectors = Vec::with_capacity(NUM_VECTORS);
    let mut parent = GENESIS_PARENT;
    for i in 0..NUM_VECTORS {
        // Vary the anchor fields predictably so Foundry can write a
        // golden-vector check without overfitting.
        let chain_id = ChainId(7);
        let height = i as u64;
        let state_root = Commitment([(i as u8).wrapping_mul(17); 32]);

        let anchor = Anchor::ecdsa(chain_id, height, state_root, parent);
        let credential = signer.sign(&anchor).expect("signing must succeed");

        let signature_bytes = match credential {
            AnchorAuthCredential::EcdsaSecp256k1 { signature } => signature,
            other => panic!("expected ECDSA credential, got {other:?}"),
        };

        vectors.push(json!({
            "chainId": chain_id.0,
            "height": height,
            "stateRoot": hex_bytes(&state_root.0),
            "parent": hex_bytes(&parent.0),
            "mac": hex_bytes(&anchor.mac),
            "signature": hex_bytes(&signature_bytes),
            "expectedSigner": hex_bytes(&signer_addr.0),
        }));

        parent = anchor.hash();
        // Note: parent above is Rust's BLAKE3-based hash. Solidity's
        // hashAnchor is keccak256 — these don't match by design (see
        // contracts/README.md "Parity model"). The Foundry test only
        // asserts `recoverSigner` correctness; per-anchor parent
        // linkage is checked separately via Rust-side tests.
    }

    let envelope = json!({
        "schema_version": 1,
        "description": "S11.5 cross-impl parity vectors: Rust ECDSA signatures verified by Solidity recoverSigner.",
        "signer_address": hex_bytes(&signer_addr.0),
        "vectors": vectors,
    });

    if let Some(dir) = out_path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).expect("create_dir_all");
        }
    }
    let pretty = serde_json::to_string_pretty(&envelope).expect("serialize");
    fs::write(&out_path, pretty).expect("write");
    println!("Wrote {NUM_VECTORS} vectors to {}", out_path.display());
    println!("Signer address: {}", hex_bytes(&signer_addr.0));
}
