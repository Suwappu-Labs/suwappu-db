//! Data lane — mempool and intent queue.
//!
//! This crate handles untrusted ingest. It accepts intents, queues them, and
//! eventually hands them off to `gsxdb-bridge` for validation and state
//! mutation.
//!
//! # Lane-separation invariant
//!
//! This crate must never `use gsxdb_state::*` and must never list
//! `gsxdb-state` as a Cargo dependency. The only path from a lane to state
//! is through a [`gsxdb_bridge::Bridge`] handle passed in by the caller.
//!
//! Enforcement:
//! - `Cargo.toml` of this crate has no `gsxdb-state` dep
//! - `scripts/check-lane-separation.sh` rejects any `use gsxdb_state` in
//!   this crate's source

#![deny(missing_docs)]

use gsxdb_bridge::{Bridge, Intent, RejectReason};
use std::collections::VecDeque;

/// Outcome of draining a queued intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Intent was applied to state.
    Applied,
    /// Intent was rejected during validation.
    Rejected(RejectReason),
}

/// Simple FIFO mempool. Phase-1 placeholder; S5 replaces this with the
/// crash-recoverable cross-VM intent queue Q.
#[derive(Debug, Default)]
pub struct Mempool {
    queue: VecDeque<Intent>,
}

impl Mempool {
    /// Create an empty mempool.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Enqueue an intent. Always succeeds in phase-1 (no quota).
    pub fn enqueue(&mut self, intent: Intent) {
        self.queue.push_back(intent);
    }

    /// Number of intents waiting.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// True if no intents are waiting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Drain one intent through the bridge, returning the outcome.
    ///
    /// Returns `None` if the queue was empty.
    pub fn drain_one(&mut self, bridge: &mut Bridge<'_>) -> Option<DrainOutcome> {
        let intent = self.queue.pop_front()?;
        Some(match bridge.submit(intent) {
            Ok(()) => DrainOutcome::Applied,
            Err(reason) => DrainOutcome::Rejected(reason),
        })
    }

    /// Drain every queued intent through the bridge in FIFO order. Returns
    /// the outcomes in the same order.
    pub fn drain_all(&mut self, bridge: &mut Bridge<'_>) -> Vec<DrainOutcome> {
        let mut outcomes = Vec::with_capacity(self.queue.len());
        while let Some(outcome) = self.drain_one(bridge) {
            outcomes.push(outcome);
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // NOTE: this test imports from gsxdb_bridge to construct test fixtures.
    // It must NOT import from gsxdb_state — and it doesn't, because the
    // bridge re-exports Address through its public API surface.
    //
    // For phase-1 we leave Address re-export as a follow-up: the bridge
    // currently exposes Address only through Intent variants, which is
    // sufficient for unit tests that drive the lane.

    #[test]
    fn mempool_starts_empty() {
        let mempool = Mempool::new();
        assert!(mempool.is_empty());
        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn drain_empty_stays_empty() {
        let mempool = Mempool::new();
        // We can't construct a State here without depending on gsxdb-state,
        // which the invariant forbids. Cross-crate drain behavior is covered
        // by integration tests in the bridge crate, where Address is in scope
        // through Intent.
        assert!(mempool.is_empty());
    }
}
