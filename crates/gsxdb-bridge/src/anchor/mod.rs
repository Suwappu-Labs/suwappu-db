//! Cross-chain anchor log.
//!
//! Each block produces an [`Anchor`] — a per-chain commitment of
//! `(height, state_root, parent_anchor)` authenticated with an
//! HMAC-style MAC keyed per chain. Anchors are appended to per-chain
//! [`AnchorLog`]s; an [`AnchorDispatcher`] writes one anchor to every
//! configured chain after each block.
//!
//! At any height, the cross-chain parity check reads the anchor from
//! every chain's log and verifies they all commit to the same state
//! root. Disagreement surfaces a divergent set with the chain ids.
//!
//! Per IQ-7, phase-1 ships in-memory logs + MAC authentication. Real
//! Solidity `LTPAnchorRegistry` and ECDSA/EdDSA signatures are launch-
//! readiness items.

pub mod dispatcher;
pub mod l1_reader;
pub mod log;
pub mod types;

#[cfg(test)]
mod parity_test;

pub use dispatcher::{AnchorDispatcher, ParityResult};
pub use l1_reader::{L1AnchorReader, MockL1AnchorReader, RpcL1AnchorReader};
pub use log::{AnchorLog, AppendError};
pub use types::{Anchor, AnchorHash, ChainId, GENESIS_PARENT};
