//! Bundle data types.

use crate::TxOutcome;
use gsxdb_state::{EvmTx, MoveTx};

/// One step within a bundle. Either a VM-shape transaction.
///
/// Both flavours decode to the same canonical `Intent` via the existing
/// `to_canonical` helpers in `gsxdb-state::vm::tx`. The bundle layer
/// preserves the VM tag so reports and tracing can attribute side
/// effects to the originating VM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundleStep {
    /// EVM-shape transaction step.
    Evm(EvmTx),
    /// Move-shape transaction step.
    Move(MoveTx),
}

/// A flat, ordered sequence of steps that commit atomically.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Bundle {
    /// Steps in execution order. The first failing step (if any) ends
    /// execution and reverts the bundle.
    pub steps: Vec<BundleStep>,
}

impl Bundle {
    /// Empty bundle. Committing one is a no-op.
    #[must_use]
    pub fn new() -> Self {
        Self { steps: Vec::new() }
    }

    /// Bundle with a single step. Convenience for the common case.
    #[must_use]
    pub fn single(step: BundleStep) -> Self {
        Self { steps: vec![step] }
    }

    /// Append a step. Builder-style.
    #[must_use]
    pub fn with(mut self, step: BundleStep) -> Self {
        self.steps.push(step);
        self
    }

    /// `true` iff the bundle has no steps.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// Step count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }
}

/// Final disposition of a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleOutcome {
    /// Every step committed. Writes are now in canonical state.
    Committed,
    /// Step at the given index rejected; the entire bundle was
    /// rolled back. Earlier steps' writes are not in canonical state.
    Reverted {
        /// Index of the step that triggered the revert.
        failed_step: usize,
    },
}

/// Bundle execution telemetry. One entry per step plus the bundle-
/// level outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleResult {
    /// Per-step outcome, one entry per attempted step. On revert, the
    /// last entry is the failing step's `Rejected(...)`. Steps after
    /// the failure aren't attempted.
    pub step_outcomes: Vec<TxOutcome>,
    /// Bundle-level outcome.
    pub outcome: BundleOutcome,
}

impl BundleResult {
    /// `true` iff the bundle committed.
    #[must_use]
    pub fn is_committed(&self) -> bool {
        matches!(self.outcome, BundleOutcome::Committed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::Address;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn empty_bundle_round_trips() {
        let b = Bundle::new();
        assert!(b.is_empty());
        assert_eq!(b.len(), 0);
    }

    #[test]
    fn single_step_bundle() {
        let b = Bundle::single(BundleStep::Evm(EvmTx {
            from: addr(1),
            to: addr(2),
            value: 100,
        }));
        assert_eq!(b.len(), 1);
        assert!(!b.is_empty());
    }

    #[test]
    fn builder_chains() {
        let b = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(1),
                to: addr(2),
                value: 10,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(2),
                recipient: addr(3),
                amount: 10,
            }));
        assert_eq!(b.len(), 2);
        match b.steps[0] {
            BundleStep::Evm(_) => {}
            BundleStep::Move(_) => panic!("step 0 should be Evm"),
        }
        match b.steps[1] {
            BundleStep::Evm(_) => panic!("step 1 should be Move"),
            BundleStep::Move(_) => {}
        }
    }

    #[test]
    fn outcome_is_committed_predicate() {
        let r = BundleResult {
            step_outcomes: vec![TxOutcome::Committed],
            outcome: BundleOutcome::Committed,
        };
        assert!(r.is_committed());

        let r = BundleResult {
            step_outcomes: vec![TxOutcome::Committed],
            outcome: BundleOutcome::Reverted { failed_step: 0 },
        };
        assert!(!r.is_committed());
    }
}
