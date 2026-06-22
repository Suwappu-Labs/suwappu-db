//! State-tree commitment.
//!
//! 256-ary trie over `Address → BalanceSlot`, with hash-based
//! commitments at every node. Per IQ-6, the commitment scheme is
//! BLAKE3 in phase-1; real Verkle (polynomial commitments over
//! banderwagon, IPA proofs) is a launch-readiness item.
//!
//! # Layout
//!
//! - [`types`] — `Node`, `Commitment`, `Proof`, `ProofStep`.
//! - [`commit`] — pure commitment functions (empty / leaf / internal).
//! - [`ops`] — `StateTree`: build, update, root, proof, verify.
//!
//! # Why 256-ary
//!
//! Verkle is canonically 256-ary. Picking the same fan-out now means
//! the commitment-scheme swap (BLAKE3 → IPA over banderwagon) is the
//! only change needed when real Verkle lands. Tree shape, depth, and
//! traversal stay the same.
//!
//! # Why phase-1 ships hash-based
//!
//! See `docs/iq/IQ-6-verkle-vs-hash-commitment.md`. The properties S6
//! must establish — determinism, change-detection, inclusion proofs —
//! are dialect-independent and verified at 10k cases against the
//! hash-based implementation. Real Verkle adds witness compression,
//! not correctness; the swap point is well-defined.

pub mod commit;
pub mod ops;
pub mod types;
pub mod verkle;

#[cfg(feature = "production-verkle")]
pub mod verkle_scheme;

pub use commit::{commit_node, Blake3Scheme, CommitmentScheme, EMPTY_COMMITMENT};
pub use ops::StateTree;
pub use types::{Commitment, Node, Proof, ProofStep};
pub use verkle::{CommitmentSchemeKind, GroupElement, IpaOpening, IpaWitness};

#[cfg(feature = "production-verkle")]
pub use verkle_scheme::BanderwagonIpaScheme;
