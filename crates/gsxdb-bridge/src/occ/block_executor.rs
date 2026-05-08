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

use crate::bundle::registry::ContractRegistry;
use crate::bundle::types::BundleStep;
use crate::bundle::CallCtx;
use crate::occ::mv_store::{MvStore, ReadSource, TxnIdx};
use crate::occ::txn::{ReadEntry, Txn, Validator, WriteEntry};
use crate::{Intent, RejectReason};
use gsxdb_state::{Address, Balance, BalanceSlot, BridgeToken, State, StateChange};
use rayon::prelude::*;

/// Internal helper: a bundle-step read might resolve against the
/// per-bundle local accumulator (intra-bundle write seen by a later
/// step) or fall through to the MV store. Only MV resolutions go into
/// the OCC read set.
enum ReadSourceForLocal {
    Local,
    Mv(ReadSource),
}

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
    pub fn execute(self, state: &mut State, block: &[Intent]) -> BlockReport {
        self.execute_with_registry(state, block, &ContractRegistry::new())
    }

    /// Like [`Self::execute`] but with a contract registry for
    /// dispatching [`Intent::Call`]. Calls whose `target` isn't in the
    /// registry fall back to a plain transfer of `value` from `caller`.
    ///
    /// # Panics
    ///
    /// Panics if the OCC re-execution loop fails to converge within
    /// `2 * block.len() + 4` iterations.
    pub fn execute_with_registry(
        self,
        state: &mut State,
        block: &[Intent],
        registry: &ContractRegistry,
    ) -> BlockReport {
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
            //
            // NOTE: Calls dispatch to the registry; the closures we hold
            // must be Send + Sync (the trait bound enforces that).
            let state_ref: &State = state;
            let mv_ref = &mv;
            let registry_ref = registry;
            let runs: Vec<(TxnIdx, Txn)> = pending
                .par_iter()
                .copied()
                .map(|idx| {
                    let intent = block[idx].clone();
                    let txn = execute_one(intent, idx, state_ref, mv_ref, registry_ref);
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
            for (idx, txn) in txns.iter().enumerate() {
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
fn execute_one(
    intent: Intent,
    idx: TxnIdx,
    state: &State,
    mv: &MvStore,
    registry: &ContractRegistry,
) -> Txn {
    match intent {
        Intent::Call {
            caller,
            target,
            value,
            calldata,
        } => execute_call(caller, target, value, &calldata, idx, state, mv, registry),
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

            let Some(new_to) = to_balance.checked_add(amount) else {
                return Txn {
                    idx,
                    read_set,
                    write_set,
                    rejected: Some(RejectReason::AmountOverflow),
                };
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

/// Speculative execution of a contract call. Looks up `target` in the
/// registry; if absent, falls back to a plain transfer of `value` from
/// `caller` to `target`. If present, runs the generator and executes
/// the resulting bundle's steps in order — atomically: any step's
/// failure clears all of this idx's writes from the MV store and
/// reports rejection.
#[allow(clippy::too_many_arguments)]
fn execute_call(
    caller: Address,
    target: Address,
    value: u128,
    calldata: &[u8],
    idx: TxnIdx,
    state: &State,
    mv: &MvStore,
    registry: &ContractRegistry,
) -> Txn {
    let mut read_set = Vec::new();
    let mut write_set = Vec::new();

    let Some(generator) = registry.get(&target) else {
        // Unknown target = plain EOA transfer of `value`.
        return execute_transfer_into(caller, target, value, idx, state, mv);
    };

    // Run the generator to get a bundle. The generator reads canonical
    // state directly (not through MV) — phase-1 simplification. Real
    // revm would read through MV via a Database adapter; that's a
    // future slice tied to real-VM integration.
    let ctx = CallCtx {
        caller,
        target,
        value,
        calldata,
        state,
        depth: 0,
    };
    let bundle = generator.generate(&ctx);

    // Execute the bundle's steps. Every step's reads/writes accumulate
    // into this single txn's read/write set. On any rejection, clear
    // all writes for this idx and return a rejected Txn — the bundle
    // is atomic.
    //
    // Bundle semantics: step N+1 must see step N's writes. The MV
    // store's `read` deliberately excludes same-idx writes (so a txn
    // doesn't see its own writes through the public API), so we
    // maintain a local intra-bundle map and consult it first.
    let mut local: std::collections::HashMap<Address, BalanceSlot> =
        std::collections::HashMap::new();

    let local_or_mv = |addr: Address,
                       local: &std::collections::HashMap<Address, BalanceSlot>|
     -> (BalanceSlot, ReadSourceForLocal) {
        if let Some(&slot) = local.get(&addr) {
            (slot, ReadSourceForLocal::Local)
        } else {
            let (slot, src) = mv.read(state, &addr, idx);
            (slot, ReadSourceForLocal::Mv(src))
        }
    };

    for step in &bundle.steps {
        let (from, to, amount) = match step {
            BundleStep::Evm(tx) => (tx.from, tx.to, tx.value),
            BundleStep::Move(tx) => (tx.signer, tx.recipient, tx.amount),
        };

        let (from_slot, from_src) = local_or_mv(from, &local);
        if let ReadSourceForLocal::Mv(src) = from_src {
            read_set.push(ReadEntry { addr: from, source: src });
        }

        let (to_slot, to_src) = local_or_mv(to, &local);
        if let ReadSourceForLocal::Mv(src) = to_src {
            read_set.push(ReadEntry { addr: to, source: src });
        }

        let from_balance = from_slot.canonical();
        let to_balance = to_slot.canonical();

        if from_balance < amount {
            mv.clear_writes(idx);
            return Txn {
                idx,
                read_set,
                write_set: Vec::new(),
                rejected: Some(RejectReason::InsufficientBalance),
            };
        }

        let Some(new_to) = to_balance.checked_add(amount) else {
            mv.clear_writes(idx);
            return Txn {
                idx,
                read_set,
                write_set: Vec::new(),
                rejected: Some(RejectReason::AmountOverflow),
            };
        };
        let new_from = from_balance - amount;

        if from == to {
            let v = BalanceSlot::new(from_balance);
            local.insert(from, v);
        } else {
            let nf = BalanceSlot::new(new_from);
            let nt = BalanceSlot::new(new_to);
            local.insert(from, nf);
            local.insert(to, nt);
        }
    }

    // Bundle succeeded: publish every accumulated address to the MV
    // store at this idx. Single write per touched address — final
    // value across all bundle steps wins.
    for (addr, slot) in &local {
        mv.write(*addr, *slot, idx);
        write_set.push(WriteEntry {
            addr: *addr,
            value: *slot,
        });
    }

    Txn {
        idx,
        read_set,
        write_set,
        rejected: None,
    }
}

/// Helper: `Call` falls through to a plain transfer when target isn't a
/// registered contract. Mirrors the `Intent::Transfer` arm of
/// [`execute_one`].
fn execute_transfer_into(
    from: Address,
    to: Address,
    amount: u128,
    idx: TxnIdx,
    state: &State,
    mv: &MvStore,
) -> Txn {
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

    let Some(new_to) = to_balance.checked_add(amount) else {
        return Txn {
            idx,
            read_set,
            write_set,
            rejected: Some(RejectReason::AmountOverflow),
        };
    };
    let new_from = from_balance - amount;

    if from == to {
        let v = BalanceSlot::new(from_balance);
        mv.write(from, v, idx);
        write_set.push(WriteEntry {
            addr: from,
            value: v,
        });
    } else {
        let nf = BalanceSlot::new(new_from);
        let nt = BalanceSlot::new(new_to);
        mv.write(from, nf, idx);
        mv.write(to, nt, idx);
        write_set.push(WriteEntry {
            addr: from,
            value: nf,
        });
        write_set.push(WriteEntry {
            addr: to,
            value: nt,
        });
    }

    Txn {
        idx,
        read_set,
        write_set,
        rejected: None,
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
        let report = BlockExecutor.execute(&mut state, &[]);
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
            &[Intent::Transfer {
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
            &[Intent::Transfer {
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
        let report = BlockExecutor.execute(&mut state, &block);

        assert_eq!(
            report.outcomes,
            vec![
                TxOutcome::Committed,
                TxOutcome::Committed,
                TxOutcome::Committed
            ]
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
        let report = BlockExecutor.execute(&mut state, &block);

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
        let _ = BlockExecutor.execute(&mut s_par, &block);

        let mut s_seq = seeded_state();
        for intent in block {
            let mut bridge = Bridge::new(&mut s_seq);
            let _ = bridge.submit(intent);
        }

        for n in 0..8u8 {
            assert_eq!(
                s_par.balance_of(&Address([n; 20])),
                s_seq.balance_of(&Address([n; 20]))
            );
        }
    }

    #[test]
    fn self_transfer_is_no_op() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute(
            &mut state,
            &[Intent::Transfer {
                from: addr(0),
                to: addr(0),
                amount: 100,
            }],
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(1_000));
    }

    // ---------- Intent::Call dispatch tests ----------

    use crate::bundle::types::BundleStep;
    use crate::bundle::{Bundle, BundleGenerator, CallCtx, ContractRegistry};
    use gsxdb_state::{EvmTx, MoveTx};
    use std::sync::Arc;

    #[test]
    fn call_to_unregistered_target_falls_back_to_transfer() {
        let mut state = seeded_state();
        let report = BlockExecutor.execute_with_registry(
            &mut state,
            &[Intent::Call {
                caller: addr(0),
                target: addr(1),
                value: 100,
                calldata: Vec::new(),
            }],
            &ContractRegistry::new(),
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
    }

    #[test]
    fn call_to_registered_forwarder_runs_its_bundle() {
        // Forwarder: receives `value` and forwards 100% to addr(99).
        // The contract's "address" addr(7) ends with the forward
        // already executed: caller debited, addr(99) credited.
        let mut state = {
            let mut s = State::default();
            let token = BridgeToken::__for_bridge_only();
            for n in 0..100u8 {
                s.apply(
                    &token,
                    &StateChange::SetBalance {
                        addr: Address([n; 20]),
                        to: Balance(1_000),
                    },
                );
            }
            s
        };

        let mut registry = ContractRegistry::new();
        let recipient = Address([99; 20]);
        let gen: Arc<dyn BundleGenerator> = Arc::new(move |ctx: &CallCtx| {
            // Forwarder bundle:
            //   step 1: caller -> contract  (value)
            //   step 2: contract -> recipient (value)
            Bundle::new()
                .with(BundleStep::Evm(EvmTx {
                    from: ctx.caller,
                    to: ctx.target,
                    value: ctx.value,
                }))
                .with(BundleStep::Evm(EvmTx {
                    from: ctx.target,
                    to: recipient,
                    value: ctx.value,
                }))
        });
        registry.register(addr(7), gen);

        let report = BlockExecutor.execute_with_registry(
            &mut state,
            &[Intent::Call {
                caller: addr(0),
                target: addr(7),
                value: 50,
                calldata: Vec::new(),
            }],
            &registry,
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(950));
        assert_eq!(state.balance_of(&addr(7)), Balance(1_000)); // net zero
        assert_eq!(state.balance_of(&recipient), Balance(1_050));
    }

    #[test]
    fn call_whose_bundle_fails_reverts_atomically() {
        // Generator emits a bundle whose 2nd step fails. The 1st step's
        // writes must NOT survive.
        let mut state = seeded_state();

        let mut registry = ContractRegistry::new();
        let gen: Arc<dyn BundleGenerator> = Arc::new(|ctx: &CallCtx| {
            Bundle::new()
                .with(BundleStep::Evm(EvmTx {
                    from: ctx.caller,
                    to: ctx.target,
                    value: 100,
                }))
                .with(BundleStep::Move(MoveTx {
                    signer: ctx.target,
                    recipient: addr(2),
                    amount: 99_999, // > balance, will fail
                }))
        });
        registry.register(addr(7), gen);

        let report = BlockExecutor.execute_with_registry(
            &mut state,
            &[Intent::Call {
                caller: addr(0),
                target: addr(7),
                value: 100,
                calldata: Vec::new(),
            }],
            &registry,
        );

        assert_eq!(
            report.outcomes,
            vec![TxOutcome::Rejected(RejectReason::InsufficientBalance)]
        );
        // No state mutation: every address remains at its seeded value.
        for n in 0..8u8 {
            assert_eq!(state.balance_of(&Address([n; 20])), Balance(1_000));
        }
    }

    #[test]
    fn call_with_evm_and_move_steps_commits_both() {
        let mut state = seeded_state();

        let mut registry = ContractRegistry::new();
        let gen: Arc<dyn BundleGenerator> = Arc::new(|ctx: &CallCtx| {
            Bundle::new()
                .with(BundleStep::Evm(EvmTx {
                    from: ctx.caller,
                    to: ctx.target,
                    value: ctx.value,
                }))
                .with(BundleStep::Move(MoveTx {
                    signer: ctx.target,
                    recipient: addr(3),
                    amount: ctx.value / 2,
                }))
        });
        registry.register(addr(7), gen);

        let report = BlockExecutor.execute_with_registry(
            &mut state,
            &[Intent::Call {
                caller: addr(0),
                target: addr(7),
                value: 100,
                calldata: Vec::new(),
            }],
            &registry,
        );

        assert_eq!(report.outcomes, vec![TxOutcome::Committed]);
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(7)), Balance(1_050)); // +100 -50
        assert_eq!(state.balance_of(&addr(3)), Balance(1_050)); // +50
    }
}
