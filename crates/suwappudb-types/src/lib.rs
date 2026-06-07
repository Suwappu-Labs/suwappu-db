//! **suwappudb-types** — frozen public-type surface for downstream
//! consumers of suwappu-db.
//!
//! This crate is a **re-export facade**. The actual type definitions
//! live in `suwappudb-state` and `suwappudb-bridge`; this crate stabilises the
//! subset that the [`INTEGRATORS.md`](https://github.com/suwappu/suwappu-db/blob/main/INTEGRATORS.md)
//! "Stability promises" table names as frozen.
//!
//! ## Stability contract
//!
//! Every type re-exported from this crate is **byte-stable** under
//! pre-1.0 minor bumps:
//!
//! - New variants on `#[non_exhaustive]` enums are **additive** and
//!   non-breaking. Match-arm exhaustiveness on a future variant is
//!   not the integrator's problem.
//! - Field additions to `#[non_exhaustive]` structs are **additive**.
//! - Field removal / rename / type-change is a **breaking** change
//!   and triggers a major bump.
//!
//! Internal traits (`MoveExecutor`, `CommitmentScheme`, `BlockStore`,
//! etc.) are NOT re-exported here — they live in the internal
//! crates and are subject to change. If you need one of those, depend
//! on the internal crate directly and accept the volatility.
//!
//! ## Versioning
//!
//! `suwappudb-types` carries its own `version` in Cargo.toml, independent
//! of the internal-crate versions. Until `v1.0.0`, the surface here is
//! stable but the internal layout may shift; from `v1.0.0` onward,
//! strict SemVer governs both.
//!
//! ## What's exported
//!
//! **State + addresses.** Identities, balances, commitments, snapshots.
//!
//! **Anchor + signature.** Cross-chain anchor records, auth schemes,
//! ECDSA verifier surface, hybrid credentials.
//!
//! **DAG + recovery.** Block-shape primitives for indexers that walk
//! the DAG.
//!
//! ## Adding to a downstream project
//!
//! ```toml
//! [dependencies]
//! suwappudb-types = { git = "https://github.com/suwappu/suwappu-db", tag = "v0.1.0-pre" }
//! ```
//!
//! Pin to a tag for reproducible builds. Workspace internals do not
//! depend on `suwappudb-types`; only downstream code does.

#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

// ===== State + addresses =====
pub use suwappudb_state::{
    Address, AccountNonce, Balance, BalanceSlot, BridgeToken, Commitment, EvmBalance, EvmNonce,
    MoveAddress, MoveCoinValue, MoveSequenceNumber, SlotError, State, StateChange,
};

/// State-tree primitives — see [`suwappudb_state::tree`] for the full
/// trait surface. Re-exported for inclusion-proof verifiers.
pub mod tree {
    pub use suwappudb_state::tree::{
        commit::CommitmentScheme, Blake3Scheme, Commitment, Node, Proof, ProofStep, StateTree,
    };
}

/// State snapshot capture / restore — see [`suwappudb_state::snapshot`].
/// `SNAPSHOT_ENTRY_BYTES` is added in S12.2; pre-merge consumers get
/// `StateSnapshot` + `SnapshotManager` only.
pub mod snapshot {
    pub use suwappudb_state::snapshot::{SnapshotManager, StateSnapshot};
}

/// DAG store + block-shape primitives — see [`suwappudb_state::dag`].
pub mod dag {
    pub use suwappudb_state::dag::{BlockHash, DagBlock, DagStore};
}

/// Metrics surface — see [`suwappudb_state::Metrics`].
pub mod metrics {
    pub use suwappudb_state::{Counter, Gauge, Histogram, Metrics, Timer};
}

// ===== Anchor + signature =====
//
// Re-exports are scoped to what's reachable on main today. The S11
// surface additions (`VerifierConfig`, `EcdsaSecp256k1Signer`,
// `AnchorSigner`, `SignerError`, `AnchorEntry`) live on their
// per-sub-pass branches; this facade picks them up when those merge,
// since the re-export sites are already prepared in `suwappudb-bridge`'s
// module root.
pub use suwappudb_bridge::anchor::{
    eth_signed_message_hash, verify_credential, verify_ecdsa, Anchor, AnchorAuthCredential,
    AnchorDispatcher, AnchorHash, AnchorLog, AppendError, ChainId, CredentialVerifyError,
    EcdsaVerifyError, EthAddress, ExpectedVerifier, L1AnchorReader, MockL1AnchorReader,
    ParityResult, RpcL1AnchorReader, Sp1PublicValues, ECDSA_SIG_LEN, GENESIS_PARENT,
};

/// `AuthScheme` discriminant enum. Wire encoding pinned at S11.4 —
/// Blake3Mac=0, Sp1ZkProof=1, EcdsaSecp256k1=2, MlDsa65Hybrid=3.
pub use suwappudb_bridge::anchor::types::AuthScheme;

#[cfg(test)]
mod tests {
    /// Smoke: the re-exported types actually link in default builds.
    #[test]
    fn frozen_surface_smoke() {
        // Just construct one of each major shape — if a re-export
        // ever breaks, this fails to compile, not at runtime.
        let _ = super::Address([0; 20]);
        let _ = super::Balance(0);
        let _ = super::Commitment([0; 32]);
        let _ = super::ChainId(1);
        let _ = super::AnchorHash([0; 32]);
        let _ = super::AuthScheme::Blake3Mac;
        let _ = super::GENESIS_PARENT;
    }
}
