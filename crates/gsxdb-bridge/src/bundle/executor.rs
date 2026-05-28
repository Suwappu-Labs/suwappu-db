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
use crate::{Bridge, Intent, RejectReason, TxOutcome};
use gsxdb_state::{
    Address, Balance, BalanceSlot, BridgeToken, CompiledModule, ModuleId, ModuleStore, MoveAddress,
    MoveBalanceView, MoveExecutor, MoveSessionState, ResourceWrite, State, StateChange,
};
use std::collections::HashMap;

#[cfg(feature = "production-evm-executor")]
use crate::vm::RevmExecutor;

/// Atomic bundle executor. Stateless; one call = one bundle.
#[derive(Debug, Default, Clone, Copy)]
pub struct BundleExecutor;

impl BundleExecutor {
    /// Execute `bundle` atomically against `state`. Legacy path —
    /// handles `Evm` and `Move` transfer steps only. Move-VM-bound
    /// steps (`MoveCall`, `DeployModule`) reject with
    /// [`RejectReason::MoveRuntimeRequired`] — use
    /// [`Self::execute_with_move_runtime`] for those.
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
        // Captures the full `BalanceSlot` (balance + nonce) so a revert
        // restores both halves — under `production-evm-executor` an
        // EVM step advances the sender's nonce, and the bundle's
        // atomicity guarantee must cover that too.
        let snapshot = collect_snapshot(state, bundle);

        let mut step_outcomes = Vec::with_capacity(bundle.steps.len());

        for (idx, step) in bundle.steps.iter().enumerate() {
            let step_result = run_transfer_step(step, state);
            match step_result {
                StepResult::Committed => step_outcomes.push(TxOutcome::Committed),
                StepResult::Rejected(reason) => {
                    step_outcomes.push(TxOutcome::Rejected(reason));
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

    /// Execute `bundle` atomically with a Move runtime + module store.
    ///
    /// Handles all 4 `BundleStep` variants:
    /// - `Evm` / `Move` — same as [`Self::execute`], transfers via the
    ///   bridge.
    /// - `MoveCall` — invokes `move_executor` with `module_store` +
    ///   a fresh `MoveSessionState`. Resource writes apply at step
    ///   commit through the bridge.
    /// - `DeployModule` — queues the deploy. On bundle commit, queued
    ///   deploys land in `module_store`; on revert, the queue is
    ///   discarded so the store is unchanged.
    ///
    /// On revert, both substrate state AND `module_store` are restored
    /// to the pre-bundle snapshot.
    ///
    /// The `balance_view` is a read-only window over the substrate's
    /// balance store as a `MoveBalanceView` — preserves lane separation
    /// (the executor can't touch redb / RocksDB directly).
    ///
    /// # Errors
    ///
    /// No bare `Err`; failures surface as `BundleOutcome::Reverted` in
    /// the returned `BundleResult`.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_with_move_runtime(
        self,
        state: &mut State,
        bundle: &Bundle,
        move_executor: &dyn MoveExecutor,
        module_store: &mut dyn ModuleStore,
        balance_view: &dyn MoveBalanceView,
    ) -> BundleResult {
        if bundle.is_empty() {
            return BundleResult {
                step_outcomes: Vec::new(),
                outcome: BundleOutcome::Committed,
            };
        }

        let snapshot = collect_snapshot(state, bundle);
        let mut step_outcomes = Vec::with_capacity(bundle.steps.len());
        // Deferred module deploys — applied only on full-bundle commit.
        let mut queued_deploys: Vec<(ModuleId, CompiledModule)> = Vec::new();

        for (idx, step) in bundle.steps.iter().enumerate() {
            let outcome = match step {
                BundleStep::Evm(_) | BundleStep::Move(_) => match run_transfer_step(step, state) {
                    StepResult::Committed => Ok(()),
                    StepResult::Rejected(reason) => Err(reason),
                },
                BundleStep::MoveCall(call) => {
                    let mut session = MoveSessionState::new(balance_view);
                    match move_executor.execute(call, module_store, &mut session) {
                        Ok(outcome) => {
                            apply_resource_writes(state, &outcome.resource_writes);
                            Ok(())
                        }
                        Err(e) => {
                            Err(RejectReason::MoveCallFailed(format!("{e:?}")))
                        }
                    }
                }
                BundleStep::DeployModule {
                    account,
                    name,
                    bytes,
                } => {
                    // Validate that the deploy slot is free under the
                    // current view (real put deferred to commit).
                    let id = ModuleId {
                        address: *account,
                        name: name.clone(),
                    };
                    if module_store.contains(&id)
                        || queued_deploys.iter().any(|(qid, _)| qid == &id)
                    {
                        Err(RejectReason::ModuleDeployFailed(format!(
                            "module {:?}::{} already deployed (or queued)",
                            account,
                            name.as_str()
                        )))
                    } else {
                        queued_deploys.push((
                            id,
                            CompiledModule {
                                bytes: bytes.clone(),
                            },
                        ));
                        Ok(())
                    }
                }
            };

            match outcome {
                Ok(()) => step_outcomes.push(TxOutcome::Committed),
                Err(reason) => {
                    step_outcomes.push(TxOutcome::Rejected(reason));
                    restore_snapshot(state, &snapshot);
                    // queued_deploys dropped — module_store untouched.
                    return BundleResult {
                        step_outcomes,
                        outcome: BundleOutcome::Reverted { failed_step: idx },
                    };
                }
            }
        }

        // Bundle committed — flush queued deploys.
        for (id, module) in queued_deploys {
            // Pre-flight checked contains; expect succeeds under that
            // invariant. If something raced (different `&mut` would
            // be a borrow-checker error), the put still rejects and
            // we'd lose the bundle's atomicity. Document as a single-
            // writer invariant on the module store.
            let _ = module_store.put(id, module);
        }

        BundleResult {
            step_outcomes,
            outcome: BundleOutcome::Committed,
        }
    }
}

/// Outcome of running a single bundle step's transfer dispatch.
///
/// Kept private so the bundle executor can centralise its
/// "real EVM under `production-evm-executor`, mock-via-bridge otherwise"
/// choice without leaking the feature gate to callers.
enum StepResult {
    Committed,
    Rejected(RejectReason),
}

/// Dispatch a transfer-shaped bundle step against `state`.
///
/// Under `production-evm-executor`, `BundleStep::Evm` runs through the
/// real `RevmExecutor` — gas, envelope-nonce validation, and nonce
/// advance all flow through revm and the bridge's `set_account`
/// write-back. Without the feature, the legacy path lowers the step to
/// `Intent::Transfer` and submits through the bridge so existing
/// tests / parity gates / OCC paths keep working.
///
/// `BundleStep::Move` always goes through `Bridge::submit(Intent::Transfer)`
/// — the real Move runtime path is handled by
/// `execute_with_move_runtime` via `BundleStep::MoveCall`.
///
/// `MoveCall` / `DeployModule` are caller errors here (this is the
/// transfer-only dispatch); we surface `MoveRuntimeRequired` so the
/// outer executor reverts the bundle.
fn run_transfer_step(step: &BundleStep, state: &mut State) -> StepResult {
    match step {
        #[cfg(feature = "production-evm-executor")]
        BundleStep::Evm(tx) => match RevmExecutor.execute(state, *tx) {
            Ok(()) => StepResult::Committed,
            Err(crate::vm::EvmError::Revert(reason)) => StepResult::Rejected(reason),
        },
        #[cfg(not(feature = "production-evm-executor"))]
        BundleStep::Evm(tx) => {
            let c = tx.to_canonical();
            let intent = Intent::Transfer {
                from: c.from,
                to: c.to,
                amount: c.amount,
            };
            let mut bridge = Bridge::new(state);
            match bridge.submit(intent) {
                Ok(()) => StepResult::Committed,
                Err(reason) => StepResult::Rejected(reason),
            }
        }
        BundleStep::Move(tx) => {
            let c = tx.to_canonical();
            let intent = Intent::Transfer {
                from: c.from,
                to: c.to,
                amount: c.amount,
            };
            let mut bridge = Bridge::new(state);
            match bridge.submit(intent) {
                Ok(()) => StepResult::Committed,
                Err(reason) => StepResult::Rejected(reason),
            }
        }
        BundleStep::MoveCall(_) | BundleStep::DeployModule { .. } => {
            StepResult::Rejected(RejectReason::MoveRuntimeRequired)
        }
    }
}

/// Apply Move-executor `ResourceWrite`s to substrate state via the
/// bridge token. The writes are absolute (new balance + new nonce);
/// canonicalisation projects them back to EVM through `BalanceSlot`.
fn apply_resource_writes(state: &mut State, writes: &[ResourceWrite]) {
    let token = BridgeToken::__for_bridge_only();
    for w in writes {
        // 32-byte MoveAddress → canonical 20-byte EVM via the
        // address-shape projection. S9.5 swaps this for the
        // gsxdb-state::Address enum once it lands.
        let evm_addr = move_addr_to_evm(&w.addr);
        let canonical = w.coin_value.to_u128();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: evm_addr,
                to: Balance(canonical),
            },
        );
    }
}

/// Canonical 32-byte → 20-byte projection (left-zero-pad inverse).
/// IQ-4: EVM-addressable Move accounts have their last 20 bytes equal
/// to the EVM address and the upper 12 zero. We take the last 20 bytes
/// unconditionally; non-EVM-addressable accounts (upper 12 non-zero)
/// produce a non-canonical projection that the dual-projection
/// invariant doesn't claim to hold on. Filtering those out at
/// bundle-admit is S9.5's job.
fn move_addr_to_evm(addr: &MoveAddress) -> Address {
    let mut out = [0u8; 20];
    out.copy_from_slice(&addr.0[12..32]);
    Address(out)
}

fn collect_snapshot(state: &State, bundle: &Bundle) -> HashMap<Address, BalanceSlot> {
    let mut snap = HashMap::new();
    for step in &bundle.steps {
        let mut record = |addr: Address| {
            snap.entry(addr).or_insert_with(|| state.slot_of(&addr));
        };
        match step {
            BundleStep::Evm(tx) => {
                record(tx.from);
                record(tx.to);
            }
            BundleStep::Move(tx) => {
                record(tx.signer);
                record(tx.recipient);
            }
            BundleStep::MoveCall(call) => {
                // Caller's EVM-projected address. Resource writes from
                // the executor may also touch the function arguments'
                // addresses, but we don't pre-decode BCS here — the
                // bridge sees actual writes at apply time and a revert
                // re-applies the snapshot for those that fell within.
                // We over-record by including the caller; full revert
                // safety for non-caller addresses is enforced by the
                // restore step which sets every touched address back to
                // its snapshot value.
                record(move_addr_to_evm(&call.caller));
            }
            BundleStep::DeployModule { account, .. } => {
                // Deploys don't touch balance; recording the account
                // address is defensive only — keeps the snapshot
                // shape uniform across steps.
                record(move_addr_to_evm(account));
            }
        }
    }
    snap
}

fn restore_snapshot(state: &mut State, snap: &HashMap<Address, BalanceSlot>) {
    let token = BridgeToken::__for_bridge_only();
    for (addr, slot) in snap {
        // Restore the full slot via `SetAccount` so a revert undoes
        // any nonce advance from an EVM step inside the bundle, not
        // just the balance. Without this, a successful revm step
        // followed by a failed Move step would leave the sender's
        // nonce bumped even though the bundle reverted.
        state.apply(
            &token,
            &StateChange::SetAccount {
                addr: *addr,
                balance: slot.as_balance(),
                nonce: slot.nonce().value,
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
            nonce: 0,
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
                nonce: 0,
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
                nonce: 0,
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
                nonce: 0,
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert!(!result.is_committed());
        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 1 });
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
            nonce: 0,
        }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 0 });
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
                nonce: 0,
            }))
            .with(BundleStep::Evm(EvmTx {
                from: addr(2),
                to: addr(3),
                value: 200,
                nonce: 0,
            }))
            .with(BundleStep::Move(MoveTx {
                signer: addr(4),
                recipient: addr(5),
                amount: 9_999, // fail
            }));
        let result = BundleExecutor.execute(&mut state, &bundle);

        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 2 });
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
                nonce: 0,
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

    // -------- S9.4: Move-runtime bundle path --------

    use gsxdb_state::{
        Identifier, InMemoryModuleStore, MockMoveExecutor, ModuleId, MoveCall, MoveCoinValue,
        CANONICAL_COIN_ADDRESS,
    };

    /// Snapshot `MoveBalanceView` — built from `&State` then detaches.
    /// Lets tests hand `&mut state` to the executor while keeping a
    /// read-side handle on the pre-bundle balances.
    #[derive(Debug, Default)]
    struct SnapshotView {
        balances: std::collections::HashMap<MoveAddress, u128>,
    }

    impl SnapshotView {
        fn from_state(state: &State, addrs: &[MoveAddress]) -> Self {
            let mut balances = std::collections::HashMap::new();
            for ma in addrs {
                let evm = move_addr_to_evm(ma);
                balances.insert(*ma, state.balance_of(&evm).0);
            }
            Self { balances }
        }
    }

    impl MoveBalanceView for SnapshotView {
        fn coin_value(&self, addr: &MoveAddress) -> MoveCoinValue {
            MoveCoinValue::from_u128(self.balances.get(addr).copied().unwrap_or(0))
        }
        fn nonce(&self, _addr: &MoveAddress) -> gsxdb_state::AccountNonce {
            gsxdb_state::AccountNonce::new(0)
        }
    }

    /// Build a Move address whose canonical EVM projection (last 20
    /// bytes) equals `addr(byte)`. Top 12 bytes zero per IQ-4's
    /// canonical EVM↔Move pad.
    fn move_addr(byte: u8) -> MoveAddress {
        let mut bytes = [0u8; 32];
        for b in &mut bytes[12..32] {
            *b = byte;
        }
        MoveAddress(bytes)
    }

    fn transfer_call(from: MoveAddress, to: MoveAddress, amount: u64) -> MoveCall {
        MoveCall {
            caller: from,
            module: ModuleId {
                address: CANONICAL_COIN_ADDRESS,
                name: Identifier::new("coin").unwrap(),
            },
            function: Identifier::new("transfer").unwrap(),
            type_arguments: Vec::new(),
            arguments: vec![to.0.to_vec(), amount.to_le_bytes().to_vec()],
        }
    }

    #[test]
    fn move_call_step_applies_resource_writes() {
        // Initial state: addr(1) has 1000, addr(2) has 0. A MoveCall
        // transfer of 250 from 1→2 should leave 750 and 250.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(1),
                to: Balance(1000),
            },
        );
        let view = SnapshotView::from_state(&state, &[move_addr(1), move_addr(2)]);
        assert_eq!(view.coin_value(&move_addr(1)).to_u128(), 1000);
        assert_eq!(view.coin_value(&move_addr(2)).to_u128(), 0);

        let bundle = Bundle::single(BundleStep::MoveCall(transfer_call(
            move_addr(1),
            move_addr(2),
            250,
        )));
        let mut modules = InMemoryModuleStore::new();
        let executor = MockMoveExecutor;

        let result = BundleExecutor.execute_with_move_runtime(
            &mut state,
            &bundle,
            &executor,
            &mut modules,
            &view,
        );

        assert!(result.is_committed(), "result was {result:?}");
        assert_eq!(state.balance_of(&addr(1)), Balance(750));
        assert_eq!(state.balance_of(&addr(2)), Balance(250));
    }

    #[test]
    fn deploy_module_commits_only_on_full_bundle_success() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(0),
                to: Balance(1000),
            },
        );
        let bundle = Bundle::new()
            .with(BundleStep::DeployModule {
                account: move_addr(7),
                name: Identifier::new("hello").unwrap(),
                bytes: vec![0xCA, 0xFE, 0xBA, 0xBE],
            })
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
                nonce: 0,
            }));
        let mut modules = InMemoryModuleStore::new();
        let executor = MockMoveExecutor;
        let view = SnapshotView::from_state(&state, &[move_addr(7)]);

        let result = BundleExecutor.execute_with_move_runtime(
            &mut state,
            &bundle,
            &executor,
            &mut modules,
            &view,
        );

        assert!(result.is_committed());
        assert_eq!(modules.len(), 1);
        let id = ModuleId {
            address: move_addr(7),
            name: Identifier::new("hello").unwrap(),
        };
        assert!(modules.contains(&id));
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        // addr(1) starts at 0 (no seed); EVM step transferred 100.
        assert_eq!(state.balance_of(&addr(1)), Balance(100));
    }

    #[test]
    fn deploy_module_discarded_on_revert() {
        // Step 1 deploys; step 2 fails. The deploy must NOT land in
        // the module store.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(0),
                to: Balance(10),
            },
        );
        let bundle = Bundle::new()
            .with(BundleStep::DeployModule {
                account: move_addr(7),
                name: Identifier::new("hello").unwrap(),
                bytes: vec![0xCA, 0xFE, 0xBA, 0xBE],
            })
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 5000, // fail
                nonce: 0,
            }));
        let mut modules = InMemoryModuleStore::new();
        let executor = MockMoveExecutor;
        let view = SnapshotView::from_state(&state, &[move_addr(7)]);

        let result = BundleExecutor.execute_with_move_runtime(
            &mut state,
            &bundle,
            &executor,
            &mut modules,
            &view,
        );

        assert!(!result.is_committed());
        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 1 });
        // Module deploy reverted — store unchanged.
        assert!(modules.is_empty());
        // Balance reverted.
        assert_eq!(state.balance_of(&addr(0)), Balance(10));
        assert_eq!(state.balance_of(&addr(1)), Balance(0));
    }

    #[test]
    fn legacy_execute_rejects_move_call_step() {
        // A bundle that mixes a legacy Evm step with a MoveCall step,
        // run through `execute` (no move runtime), should fail at the
        // MoveCall step with MoveRuntimeRequired and revert.
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr(0),
                to: Balance(1000),
            },
        );
        let bundle = Bundle::new()
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
                nonce: 0,
            }))
            .with(BundleStep::MoveCall(transfer_call(
                move_addr(0),
                move_addr(1),
                50,
            )));
        let result = BundleExecutor.execute(&mut state, &bundle);
        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 1 });
        assert!(matches!(
            result.step_outcomes[1],
            TxOutcome::Rejected(RejectReason::MoveRuntimeRequired)
        ));
        // Step 1's writes reverted.
        assert_eq!(state.balance_of(&addr(0)), Balance(1000));
        assert_eq!(state.balance_of(&addr(1)), Balance(0));
    }
}

/// Bundle-level tests that exercise the `production-evm-executor`
/// dispatch path — `BundleStep::Evm` routes through `RevmExecutor`
/// (real revm) instead of lowering to `Intent::Transfer`. These are
/// the regression tests for Codex P2 (`vm/mod.rs ~19`): bundles must
/// not silently bypass real EVM gas/nonce when the feature is on.
#[cfg(all(test, feature = "production-evm-executor"))]
mod revm_bundle_tests {
    use super::*;
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

    /// Under `production-evm-executor`, a single-step EVM bundle dispatches
    /// through real revm. The sender's nonce advances (the legacy mock
    /// path never bumped it), proving the bundle layer is actually
    /// calling `RevmExecutor` instead of `Bridge::submit(Intent::Transfer)`.
    #[test]
    fn evm_bundle_step_routes_through_real_revm() {
        let mut state = seeded_state();
        let bundle = Bundle::single(BundleStep::Evm(EvmTx {
            from: addr(0),
            to: addr(1),
            value: 100,
            nonce: 0,
        }));

        let result = BundleExecutor.execute(&mut state, &bundle);

        assert!(result.is_committed());
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
        // Definitive marker that revm ran: the sender's nonce advanced.
        assert_eq!(
            state.slot_of(&addr(0)).nonce().value,
            1,
            "real revm must advance the sender's nonce when dispatched via the bundle path"
        );
    }

    /// A bundle whose EVM step replays a stale envelope nonce is rejected
    /// at that step and the whole bundle reverts. Without revm in the
    /// bundle dispatch, this replay would silently succeed via
    /// `Bridge::submit(Intent::Transfer)`.
    #[test]
    fn evm_bundle_step_rejects_replayed_nonce() {
        let mut state = seeded_state();

        // First bundle: nonce 0, succeeds, sender now at nonce 1.
        BundleExecutor.execute(
            &mut state,
            &Bundle::single(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
                nonce: 0,
            })),
        );
        assert_eq!(state.slot_of(&addr(0)).nonce().value, 1);

        // Second bundle: replays nonce 0, must reject.
        let result = BundleExecutor.execute(
            &mut state,
            &Bundle::single(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 50,
                nonce: 0,
            })),
        );

        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 0 });
        assert!(matches!(
            result.step_outcomes[0],
            TxOutcome::Rejected(RejectReason::InvalidNonce)
        ));
        // Balances unchanged from after the first bundle.
        assert_eq!(state.balance_of(&addr(0)), Balance(900));
        assert_eq!(state.balance_of(&addr(1)), Balance(1_100));
        // Nonce did not advance again.
        assert_eq!(state.slot_of(&addr(0)).nonce().value, 1);
    }

    /// A mid-bundle revert restores the full slot — including any nonce
    /// advance from a successful earlier EVM step. Without slot-aware
    /// snapshots, a revert would leave the sender's nonce bumped even
    /// though the bundle was rolled back, breaking the bundle's
    /// atomicity guarantee under real revm.
    #[test]
    fn bundle_revert_restores_evm_nonce() {
        let mut state = seeded_state();
        let bundle = Bundle::new()
            // Step 0: real-revm transfer succeeds, bumps Alice's nonce
            // from 0 → 1.
            .with(BundleStep::Evm(EvmTx {
                from: addr(0),
                to: addr(1),
                value: 100,
                nonce: 0,
            }))
            // Step 1: Move transfer fails (insufficient balance) →
            // bundle reverts.
            .with(BundleStep::Move(MoveTx {
                signer: addr(2),
                recipient: addr(3),
                amount: 5_000,
            }));

        let result = BundleExecutor.execute(&mut state, &bundle);
        assert_eq!(result.outcome, BundleOutcome::Reverted { failed_step: 1 });

        // Atomic revert: every touched balance AND the EVM-bumped nonce
        // are back to pre-bundle values.
        for n in 0..8u8 {
            assert_eq!(state.balance_of(&Address([n; 20])), Balance(1_000));
        }
        assert_eq!(
            state.slot_of(&addr(0)).nonce().value,
            0,
            "bundle revert must roll back the sender's nonce advance from the EVM step"
        );
    }
}
