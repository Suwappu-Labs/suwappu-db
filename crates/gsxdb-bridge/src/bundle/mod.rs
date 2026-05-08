//! Cross-VM intent bundles.
//!
//! A [`Bundle`] is a flat ordered sequence of [`BundleStep`]s — each
//! step is a VM-shape transaction. Bundles commit atomically: if any
//! step rejects, the entire bundle reverts. This is the operational
//! form of the chain's "EVM contract calls a Move resource" claim:
//! contracts produce bundles, bundles commit or revert as a unit, and
//! cross-VM operations are just bundles whose steps mix VM flavours.
//!
//! # Layout
//!
//! - [`types`] — `Bundle`, `BundleStep`, `BundleResult`, `BundleOutcome`.
//! - [`executor`] — `BundleExecutor::execute` runs a bundle atomically
//!   against `&mut State`.
//! - [`registry`] — `ContractRegistry` maps `Address` to a closure
//!   that generates a bundle in response to a contract call. Mock-
//!   contract substrate pending real-VM integration.
//!
//! # Atomicity
//!
//! Atomicity is enforced by save-and-restore: `BundleExecutor` snapshots
//! every address it's about to touch before the first step. On revert,
//! it writes the snapshots back. Reads through the bundle still see
//! the running speculative state (so step `n+1` sees step `n`'s writes
//! when step `n` succeeded).
//!
//! # Per IQ-3
//!
//! Phase-1 contracts are Rust closures, not real bytecode. The
//! `ContractRegistry` is the substrate that real-revm and real-Move
//! drop into when those land. The bundle/atomicity machinery here is
//! independent of which VM produces the bundle.

pub mod executor;
pub mod registry;
pub mod types;

pub use executor::BundleExecutor;
pub use registry::{BundleGenerator, CallCtx, ContractRegistry};
pub use types::{Bundle, BundleOutcome, BundleResult, BundleStep};
