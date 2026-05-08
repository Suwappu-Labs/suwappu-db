//! VM-shape executors.
//!
//! Mutate canonical [`gsxdb_state::State`] through the bridge in
//! response to VM-typed transactions ([`gsxdb_state::EvmTx`],
//! [`gsxdb_state::MoveTx`]). All write paths flow through
//! [`crate::Bridge::submit`] so the `BridgeToken` capability gate
//! remains the single mutation entry point.
//!
//! Phase-1 ships *mock* executors — faithful reimplementations of the
//! semantics each VM would apply, written in plain Rust. Real revm /
//! Move-VM integration lands in a follow-up sprint per IQ-2.

pub mod executor;

pub use executor::{EvmError, MockEvm, MockMove, MoveError};
