//! Atomic bundle executor.
//!
//! Executes a [`Bundle`] step-by-step against `&mut State`. If every
//! step succeeds, writes stay in canonical state. If any step fails,
//! all of the bundle's writes are rolled back to the pre-bundle
//! snapshot.
//!
//! # Implementation
//!
//! Save-and-restore at the bundle boundary. Before the first step we
//! collect a snapshot map keyed by every address each step touches,
//! reading the current canonical balance via `State::balance_of`. Then
//! we run each step through `Bridge::submit`. On failure, we re-apply
//! the snapshot map. On success, the snapshot is discarded and the
//! canonical state already reflects the post-bundle balances.
//!
//! # Why not the OCC MV layer
//!
//! The OCC machinery in `crate::occ` is block-level — it manages
//! parallel speculative execution across the txns in a block. A
//! bundle is a single OCC tx-index from the block's perspective; its
//! internal atomicity is independent of OCC and uses a simpler
//! save-and-restore model. Block-level integration (where a bundle
//! becomes one OCC unit) lands in the next slice.

use crate::bundle::types::{Bundle, BundleOutcome, BundleResult, BundleStep};
use crate::{Bridge, Intent, TxOutcome};
use gsxdb_state::{Address, Balance, BridgeToken, State, StateChange};
use std::collections::HashMap;

/// Atomic bundle executor. Stateless; one call = one bundle.
#[derive(Debug, Default, Clone, Copy)]
pub struct BundleExecutor;

impl BundleExecutor {
    /// Execute `bundle` atomically against `state`.
    ///
    /// Returns a [`BundleResult`] with per-step outcomes and the
    /// overall commit/revert disposition. On revert, `state` is left
    /// exactly as it was before the call.
    pub fn execute(self, state: &mut State, bundle: &Bundle) -> BundleResult {
        if bundle.is_empty() {
            return BundleResult {
                step_outcomes: Vec::new(),
                outcome: BundleOutcome::Committed,
            };
        }

        // Pre-bundle snapshot: every address any step might touch.
        // We over-approximate: snapshot every address mentioned in
        // any step, regardless of whether that step ends up executing.
        let snapshot = collect_snapshot(state, bundle);

        let mut step_outcomes = Vec::with_capacity(bundle.steps.len());

        for (idx, step) in bundle.steps.iter().enumerate() {
            let intent = step_to_intent(*step);
            let mut bridge = Bridge::new(state);
            match bridge.submit(intent) {
                Ok(()) => step_outcomes.push(TxOutcome::Committed),
                Err(reason) => {
                    step_outcomes.push(TxOutcome::Rejected(reason.clone()));
                    restore_snapshot(state, &snapshot);
                    return BundleResult {
                        step_outcomes,
                        outcome: BundleOutcome::Reverted { failed_step: idx },
                    };
                }
            }
        }

        BundleResult {
            step_outcomes,
            outcome: BundleOutcome::Committed,
        }
    }
}

fn step_to_intent(step: BundleStep) -> Intent {
    match step {
        BundleStep::Evm(tx) => {
            let c = tx.to_canonical();
            Intent::Transfer {
                from: c.from,
                to: c.to,
                amount: c.amount,
            }
        }
        BundleStep::Move(tx) => {
            let c = tx.to_canonical();
            Intent::Transfer {
                from: c.from,
                to: c.to,
                amount: c.amount,
            }
        }
    }
}

fn collect_snapshot(state: &State, bundle: &Bundle) -> HashMap<Address, Balance> {
    let mut snap = HashMap::new();
    for step in &bundle.steps {
        let (a, b) = match step {
            BundleStep::Evm(tx) => (tx.from, tx.to),
            BundleStep::Move(tx) => (tx.signer, tx.recipient),
        };
        snap.entry(a).or_insert_with(|| state.balance_of(&a));
        snap.entry(b).or_insert_with(|| state.balance_of(&b));
    }
    snap
}

fn restore_snapshot(state: &mut State, snap: &HashMap<Address, Balance>) {
    let token = BridgeToken::__for_bridge_only();
    for (addr, balance) in snap {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: *addr,
                to: *balance,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RejectReason;
    use gsxdb_state::{EvmTx, MoveTx};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn seeded_state() -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        for n in 0..8u8 {
            state.apply(
                &token,
                &StateChange::SetBalance {
                    addr: Address([n; 20]),
                    to: Balance(1_000),
                },
            );
        }
        state
    }

    #[test]
    fn empty_bundle_commits_no_op() {
        let mut state = seeded_state();
        let result = BundleExecutor.execute(&mut state, &Bundle::new());
        assert!(result.is_committed());
        assert_eq!(result.step_outcomes.len(), 0);
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
    }

    #[test]
    fn single_evm_step_commits() {
        let mut state = seeded_state();
        let bundle = Bundle::single(BundleStep::Evm(EvmTx {
            from: addr(0),
            to: addr(1),
            value: 100,
        }));
        let result = BundleExecutor.execute(&mut state, &bundle);
        assert!(result.is_committed());
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
    }

    #[test]
    fn single_move_step_commits() {
        let mut state = seeded_state();
        let bundle = Bundle::single(BundleStep::Move(MoveTx {
            signer: addr(0),
            recipient: addr(1),
            amount: 100,
        }));
        let result = BundleExecutor.execute(&mut state, &bundle);
        assert!(result.is_committed());
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
    }

    #[test]
    fn cross_vm_two_step_bundle_commits() {
        // EVM step then Move step. Both must run, and step 2 sees
        // step 1's writes.
        let mut state = seeded_state();
        let bundle = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(1),
                recipient: addr(2),
                amount: 50,
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);
        assert!(result.is_committed());
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_050)); // 1000 + 100 - 50
        assert_eq!(state.balance_of(&addr(2)), Balance(1_050));
    }

    #[test]
    fn mid_bundle_revert_restores_state() {
        // Step 1 commits, step 2 fails (insufficient balance), step 3
        // never runs. Bundle reverts to pre-bundle state.
        let mut state = seeded_state();
        let bundle = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(2),
                recipient: addr(3),
                amount: 5_000, // > 1000 balance
            }))
            .with(BundleStep::Evm(EvmTx {
                from: addr(4),
                to: addr(5),
                value: 1,
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert!(!result.is_committed());
        assert_eq!(
            result.outcome,
            BundleOutcome::Reverted { failed_step: 1 }
        );
        assert_eq!(result.step_outcomes.len(), 2); // step 3 never tried
        assert_eq!(result.step_outcomes[0], TxOutcome::Committed);
        assert_eq!(
            result.step_outcomes[1],
            TxOutcome::Rejected(RejectReason::InsufficientBalance)
        );

        // Every address is back to its seeded value.
        for n in 0..8u8 {
            assert_eq!(state.balance_of(&Address([n; 20])), Balance(1_000));
        }
    }

    #[test]
    fn first_step_revert_restores_state() {
        let mut state = seeded_state();
        let bundle = Bundle::single(BundleStep::Evm(EvmTx {
            from: addr(0),
            to: addr(1),
            value: 5_000,
        }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert_eq!(
            result.outcome,
            BundleOutcome::Reverted { failed_step: 0 }
        );
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_000));
    }

    #[test]
    fn last_step_revert_restores_all_earlier_writes() {
        // 3-step bundle, step 3 fails. Steps 1 and 2 had committed
        // through Bridge::submit; revert must undo both.
        let mut state = seeded_state();
        let bundle = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
            }))
            .with(BundleStep::Evm(EvmTx {
                from: addr(2),
                to: addr(3),
                value: 200,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(4),
                recipient: addr(5),
                amount: 9_999, // fail
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert_eq!(
            result.outcome,
            BundleOutcome::Reverted { failed_step: 2 }
        );
        for n in 0..8u8 {
            assert_eq!(
                state.balance_of(&Address([n; 20])),
                Balance(1_000),
                "addr {n} should be restored"
            );
        }
    }

    #[test]
    fn step_outcomes_record_each_attempted_step() {
        let mut state = seeded_state();
        let bundle = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 50,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(2),
                recipient: addr(3),
                amount: 50,
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);
        assert!(result.is_committed());
        assert_eq!(result.step_outcomes.len(), 2);
        assert_eq!(result.step_outcomes[0], TxOutcome::Committed);
        assert_eq!(result.step_outcomes[1], TxOutcome::Committed);
    }
}
