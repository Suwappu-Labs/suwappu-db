//! L2 state synchronization from op-reth.
//!
//! Syncs EVM balance and nonce state from the L2 canonical state
//! (op-reth) into gsxdb's redb tables (`evm_storage`, `evm_nonces`).
//! This allows gsxdb to validate that its EVM projection matches
//! the authoritative L2 state.

pub mod l2;

pub use l2::{L2StateSyncer, L2SyncConfig};
