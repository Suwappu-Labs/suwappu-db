//! Concurrent execution + MVCC + OCC, Aptos Block-STM style.
//!
//! Phase-1 scope: balance-only, per-address granularity, ordered tx
//! indices. Sui-style object parallelism is out of scope (see IQ-3
//! decision context).
//!
//! # Layout
//!
//! - [`mv_store`] — versioned read/write store sitting in front of the
//!   canonical [`gsxdb_state::State`]. Holds per-txn writes keyed by
//!   `(Address, TxnIdx)`.
//! - [`txn`] — per-transaction read/write set tracking and the
//!   conflict validator.
//! - [`block_executor`] — entry point: run a block of intents in
//!   parallel, validate, retry, consolidate to canonical state.
//!
//! All state mutation still funnels through [`crate::Bridge::submit`]
//! at consolidation time, preserving the `BridgeToken` capability gate.
//!
//! # Determinism
//!
//! Outcomes depend only on the tx-index order, not on the rayon
//! thread schedule. The validator is a pure function of the recorded
//! read/write sets; re-execution is deterministic per-txn given the
//! same incoming versions.
//!
//! # References
//!
//! Block-STM: <https://arxiv.org/abs/2203.06871>

pub mod block_executor;
pub mod mv_store;
#[cfg(feature = "production-evm-executor")]
pub(crate) mod revm_db;
pub mod txn;

pub use block_executor::{BlockExecutor, BlockReport, TxOutcome};
pub use mv_store::{MvStore, ReadSource, TxnIdx};
pub use txn::{Txn, Validator};
