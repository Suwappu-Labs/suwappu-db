//! Block executor — Aptos Block-STM in shape, scoped to balance-only
//! intents.
//!
//! # Algorithm
//!
//! 1. Build an [`MvStore`] over the block.
//! 2. Speculatively execute every txn in parallel (rayon). Each txn
//!    reads/writes against the MV store and records its read set.
//! 3. Validate every txn against the post-execution MV state. Any
//!    txn whose read set is now stale is marked for re-execution.
//! 4. Re-execute aborted txns (clearing their writes first), validate
//!    again. Iterate until no aborts.
//! 5. Consolidate: walk the MV store at "highest version per address"
//!    and apply the resulting writes through [`Bridge::submit`] as a
//!    sequence of canonical transfers OR direct state changes.
//!
//! # Why we re-derive transfers from the MV writes at consolidation
//!
//! The MV store ends up with the post-block balance for each touched
//! address. We could just write each `(addr, slot)` directly as a
//! `StateChange::SetBalance`. That bypasses the bridge's intent-level
//! validation, but at consolidation time the speculative execution has
//! already done that validation per txn — the only role of the bridge
//! call here is to go through the [`BridgeToken`] capability gate.
//!
//! We use [`gsxdb_state::State::apply`] directly with a fresh token,
//! one `SetBalance` per touched address. Phase-1 simplification; S5
//! revisits when contract semantics matter.
//!
//! # Iteration cap
//!
//! Block-STM proves logarithmic iterations under random workloads but
//! a pathological input could in principle loop. We cap iterations at
//! `2 * block_len + 4` and panic if exceeded — that signals an
//! algorithmic bug, not bad input.

use crate::occ::mv_store::{MvStore, TxnIdx};
use crate::occ::txn::{ReadEntry, Txn, Validator, WriteEntry};
use crate::{Intent, RejectReason};
use gsxdb_state::{Balance, BalanceSlot, BridgeToken, State, StateChange};
use rayon::prelude::*;

/// Per-tx outcome reported by the block executor.
///
/// Not `Copy` — `RejectReason` carries no payload today but is not
/// `Copy` either, leaving room to grow without an API break.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxOutcome {
    /// Tx executed and its writes were consolidated.
    Committed,
    /// Tx's domain logic rejected. No writes consolidated.
    Rejected(RejectReason),
}

/// Telemetry returned by [`BlockExecutor::execute`].
#[derive(Debug, Clone)]
pub struct BlockReport {
    /// One outcome per input intent, in input order.
    pub outcomes: Vec<TxOutcome>,
    /// Number of validation iterations (1 = no aborts).
    pub iterations: usize,
    /// Total number of re-executions across all txns.
    pub aborts: usize,
}

/// Block executor. Stateless; one call = one block.
#[derive(Debug, Default, Clone, Copy)]
pub struct BlockExecutor;

impl BlockExecutor {
    /// Execute `block` against `state` with parallel CE-MVCC OCC.
    ///
    /// On return, `state` reflects the post-block consolidated balances
    /// and the [`BlockReport`] tells the caller per-tx outcomes plus
    /// scheduler telemetry.
    ///
    /// The dual-projection invariant holds at every point because all
    /// writes still flow through [`State::apply`] under a
    /// [`BridgeToken`]; the MV store layers above the canonical state
    /// without altering its shape.
    ///
    /// # Panics
    ///
    /// Panics if the OCC re-execution loop fails to converge within
    /// `2 * block.len() + 4` iterations. This indicates an algorithmic
    /// bug, not bad input.
    pub fn execute(self, state: &mut State, block: Vec<Intent>) -> BlockReport {
        let n = block.len();
        if n == 0 {
            return BlockReport {
                outcomes: Vec::new(),
                iterations: 0,
                aborts: 0,
            };
        }

        let mv = MvStore::new();
        // Per-txn slot for the latest recorded run.
        let mut txns: Vec<Txn> = (0..n)
            .map(|idx| Txn {
                idx,
                ..Default::default()
            })
            .collect();
        // Track which indices need to (re-)execute this iteration.
        let mut pending: Vec<TxnIdx> = (0..n).collect();
        let cap = 2 * n + 4;
        let mut iterations = 0usize;
        let mut aborts = 0usize;

        while !pending.is_empty() {
            iterations += 1;
            assert!(
                iterations <= cap,
                "BlockExecutor: re-execution loop exceeded {cap} iterations \
                 — algorithmic bug or pathological input"
            );

            // Re-execution pass. rayon scope — each pending txn runs
            // independently; writes to the MV store are atomic per call.
            let block_ref = &block;
            let state_ref: &State = state;
            let mv_ref = &mv;
            let runs: Vec<(TxnIdx, Txn)> = pending
                .par_iter()
                .copied()
                .map(|idx| {
                    let intent = block_ref[idx];
                    let txn = execute_one(intent, idx, state_ref, mv_ref);
                    (idx, txn)
                })
                .collect();

            for (idx, txn) in runs {
                txns[idx] = txn;
            }

            // Validation pass. Sequential — cheap, and avoids a
            // multi-thread re-entry into MvStore's lock during
            // back-to-back validates.
            let mut next_pending = Vec::new();
            for idx in 0..n {
                let txn = &txns[idx];
                if !Validator.is_valid(txn, &mv) {
                    // Stale reads: clear writes, re-execute next iter.
                    mv.clear_writes(idx);
                    next_pending.push(idx);
                    aborts += 1;
                }
            }
            pending = next_pending;
        }

        // Consolidation: walk the MV store at highest version per
        // address and apply through the bridge token.
        let final_writes = mv.finalise();
        let token = BridgeToken::__for_bridge_only();
        for (addr, slot) in final_writes {
            state.apply(
                &token,
                &StateChange::SetBalance {
                    addr,
                    to: Balance(slot.canonical()),
                },
            );
        }

        let outcomes = txns
            .into_iter()
            .map(|t| match t.rejected {
                None => TxOutcome::Committed,
                Some(reason) => TxOutcome::Rejected(reason),
            })
            .collect();

        BlockReport {
            outcomes,
            iterations,
            aborts,
        }
    }
}

/// Speculative execution of one intent. Pure function of the input MV
/// view; produces a [`Txn`] with read/write sets recorded.
fn execute_one(intent: Intent, idx: TxnIdx, state: &State, mv: &MvStore) -> Txn {
    match intent {
        Intent::Transfer { from, to, amount } => {
            let mut read_set = Vec::new();
            let mut write_set = Vec::new();

            let (from_slot, from_src) = mv.read(state, &from, idx);
            read_set.push(ReadEntry {
                addr: from,
                source: from_src,
            });

            let (to_slot, to_src) = mv.read(state, &to, idx);
            read_set.push(ReadEntry {
                addr: to,
                source: to_src,
            });

            let from_balance = from_slot.canonical();
            let to_balance = to_slot.canonical();

            if from_balance < amount {
                return Txn {
                    idx,
                    read_set,
                    write_set,
                    rejected: Some(RejectReason::InsufficientBalance),
                };
            }

            let new_to = match to_balance.checked_add(amount) {
                Some(v) => v,
                None => {
                    return Txn {
                        idx,
                        read_set,
                        write_set,
                        rejected: Some(RejectReason::AmountOverflow),
                    };
                }
            };
            let new_from = from_balance - amount;

            // Self-transfer special case: writing both legs would
            // overwrite the same address twice in different orders
            // depending on (from, to). The canonical resolution: a
            // self-transfer of amount X results in net zero change.
            if from == to {
                let new_value = BalanceSlot::new(from_balance);
                mv.write(from, new_value, idx);
                write_set.push(WriteEntry {
                    addr: from,
                    value: new_value,
                });
            } else {
                let new_from_slot = BalanceSlot::new(new_from);
                let new_to_slot = BalanceSlot::new(new_to);
                mv.write(from, new_from_slot, idx);
                mv.write(to, new_to_slot, idx);
                write_set.push(WriteEntry {
                    addr: from,
                    value: new_from_slot,
                });
                write_set.push(WriteEntry {
                    addr: to,
                    value: new_to_slot,
                });
            }

            Txn {
                idx,
                read_set,
                write_set,
                rejected: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bridge;
    use gsxdb_state::Address;

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
    fn empty_block_is_noop() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute(&mut state, Vec::new());
        assert!(report.outcomes.is_empty());
        assert_eq!(report.iterations, 0);
        assert_eq!(report.aborts, 0);
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
    }

    #[test]
    fn single_transfer_commits() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute(
            &mut state,
            vec![Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 100,
            }],
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
    }

    #[test]
    fn rejected_transfer_leaves_state_untouched() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute(
            &mut state,
            vec![Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 5_000, // > balance (1000)
            }],
        );

        assert_eq!(
            report.outcomes,
            vec![TxOutcome::Rejected(RejectReason::InsufficientBalance)]
        );
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_000));
    }

    #[test]
    fn parallel_disjoint_transfers_commit_in_one_iteration() {
        let mut state = seeded_state();
        let block = vec![
            Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 10,
            },
            Intent::Transfer {
                from: addr(2),
                to: addr(3),
                amount: 20,
            },
            Intent::Transfer {
                from: addr(4),
                to: addr(5),
                amount: 30,
            },
        ];
        let report = BlockExecutor.execute(&mut state, block);

        assert_eq!(
            report.outcomes,
            vec![TxOutcome::Committed, TxOutcome::Committed, TxOutcome::Committed]
        );
        assert_eq!(report.iterations, 1);
        assert_eq!(report.aborts, 0);

        assert_eq!(state.balance_of(&addr(0)), Balance(990));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_010));
        assert_eq!(state.balance_of(&addr(2)), Balance(980));
        assert_eq!(state.balance_of(&addr(3)), Balance(1_020));
        assert_eq!(state.balance_of(&addr(4)), Balance(970));
        assert_eq!(state.balance_of(&addr(5)), Balance(1_030));
    }

    #[test]
    fn conflicting_transfers_serialize_via_re_execution() {
        // Two txs that both debit addr(0). Block-STM serializes them
        // by tx-index: tx 0 commits first, tx 1 sees the post-tx-0
        // state and either succeeds or fails accordingly.
        let mut state = seeded_state();
        let block = vec![
            Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 600,
            },
            Intent::Transfer {
                from: addr(0),
                to: addr(2),
                amount: 500,
            },
        ];
        let report = BlockExecutor.execute(&mut state, block);

        // tx 0 succeeds; tx 1 may have read stale value from snapshot
        // first time, retries, sees post-tx-0 balance (400) which is
        // less than 500, rejects.
        assert_eq!(report.outcomes[0], TxOutcome::Committed);
        assert_eq!(
            report.outcomes[1],
            TxOutcome::Rejected(RejectReason::InsufficientBalance)
        );

        assert_eq!(state.balance_of(&addr(0)), Balance(400));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_600));
        assert_eq!(state.balance_of(&addr(2)), Balance(1_000));
    }

    #[test]
    fn parallel_equals_sequential_for_disjoint_block() {
        // Same input, applied two ways, must produce identical state.
        let block = vec![
            Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: 10,
            },
            Intent::Transfer {
                from: addr(2),
                to: addr(3),
                amount: 20,
            },
        ];

        let mut s_par = seeded_state();
        let _ = BlockExecutor.execute(&mut s_par, block.clone());

        let mut s_seq = seeded_state();
        for intent in block {
            let mut bridge = Bridge::new(&mut s_seq);
            let _ = bridge.submit(intent);
        }

        for n in 0..8u8 {
            assert_eq!(s_par.balance_of(&Address([n; 20])), s_seq.balance_of(&Address([n; 20])));
        }
    }

    #[test]
    fn self_transfer_is_no_op() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute(
            &mut state,
            vec![Intent::Transfer {
                from: addr(0),
                to: addr(0),
                amount: 100,
            }],
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
    }
}
