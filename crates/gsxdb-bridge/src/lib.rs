//! The bridge between `gsxdb-lane` (untrusted data ingest) and `gsxdb-state`
//! (authoritative state).
//!
//! Lane code submits [`Intent`]s. The bridge validates them — signature
//! checks, OCC conflict checks, balance checks — and on success produces a
//! [`gsxdb_state::StateChange`] that is applied through a `BridgeToken` only
//! this crate can mint.
//!
//! Phase-1 implementation is intentionally thin. S3 (CE-MVCC + OCC) and S4
//! (cross-VM intent queue) extend [`Bridge::submit`] with real validation.

#![deny(missing_docs)]
// HARDENING rec 2.3 — Wormhole 2022 ($326M) traced to a deprecated
// `load_instruction_at` accepting a forged sysvar account. The
// gsxdb-bridge crate is the only capability-gated mutation surface;
// deny deprecated functions at the crate level so a future
// "legacy verify" or "unchecked variant" can't slip past review.
// Halborn: https://www.halborn.com/blog/post/explained-the-wormhole-hack-february-2022
#![deny(deprecated)]

pub mod anchor;
pub mod bundle;
pub mod occ;
pub mod recovery;
pub mod sync;
pub mod telemetry;
pub mod vm;

pub use anchor::{
    Anchor, AnchorDispatcher, AnchorHash, AnchorLog, AppendError, ChainId, ParityResult,
    GENESIS_PARENT,
};
pub use bundle::{
    Bundle, BundleExecutor, BundleGenerator, BundleOutcome, BundleResult, BundleStep, CallCtx,
    ContractRegistry,
};
pub use occ::{BlockExecutor, BlockReport, TxOutcome};
pub use recovery::{
    replay, Block, BlockHash, BlockStore, InMemoryBlockStore, RecoveryError, RedbBlockStore,
};
pub use sync::{L2StateSyncer, L2SyncConfig};
pub use telemetry::{
    record_block_metrics, record_parity_metrics, record_state_metrics, AnchorTimer, BlockTimer,
    ParityTimer,
};
pub use vm::{EvmError, MockEvm, MockMove, MoveError};

use gsxdb_state::{Address, Balance, BalanceSlot, BridgeToken, State, StateChange};

/// An untrusted intent submitted from the lane.
///
/// Not `Copy` — `Call` carries a `Vec<u8>` for calldata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Intent {
    /// Transfer `amount` from `from` to `to`. Source must hold ≥ `amount`.
    Transfer {
        /// Source address.
        from: Address,
        /// Destination address.
        to: Address,
        /// Amount in wei-equivalent units.
        amount: u128,
    },
    /// Invoke the contract at `target` with `calldata` and `value`. Dispatched
    /// through the [`ContractRegistry`] at block-execution time. If `target`
    /// isn't a registered contract, the call falls back to a plain transfer
    /// of `value` from `caller` to `target`.
    ///
    /// Bridge-level [`Bridge::submit`] does NOT handle `Call` — only the
    /// block executor does, because dispatch needs registry access. Calling
    /// `Bridge::submit` with a `Call` returns
    /// [`RejectReason::CallRequiresRegistry`].
    Call {
        /// Originating account (EOA for top-level, parent contract for sub-calls).
        caller: Address,
        /// Contract address being called. May or may not be a registered contract.
        target: Address,
        /// Native value passed with the call.
        value: u128,
        /// Opaque payload. Phase-1 mock contracts agree on shape; real
        /// revm parses ABI.
        calldata: Vec<u8>,
    },
    /// Deploy a Move module to the substrate's [`ModuleStore`]. S9.3.
    ///
    /// The module is keyed by `(account, name)`; re-deploys are rejected
    /// (`ModuleStoreError::AlreadyExists`). Upgrades are a separate
    /// surface — deferred per `docs/spec/move-execution.md` open
    /// question on Aptos `compatible` upgrade policy.
    ///
    /// `bytes` is opaque BCS-encoded Move bytecode. Verifier runs at
    /// deploy time in S9.5 (currently passthrough; the verifier needs
    /// `move-bytecode-verifier`). The lane carries any byte sequence;
    /// the bundle executor + verifier reject malformed bytecode.
    DeployModule {
        /// Originating Move account (32-byte). Must match the
        /// `address` field of the deployed module's `ModuleId`.
        account: gsxdb_state::MoveAddress,
        /// Module name. Validated as a Move [`Identifier`] before
        /// reaching the lane.
        name: gsxdb_state::Identifier,
        /// Opaque BCS-encoded bytecode.
        bytes: Vec<u8>,
    },
}

/// Reasons an intent can be rejected during validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Balance below the requested withdrawal: source side of a
    /// transfer (`Bridge::submit`), or the target of a protocol
    /// debit (`Bridge::debit`).
    InsufficientBalance,
    /// Balance arithmetic overflowed `u128`: destination side of a
    /// transfer (`Bridge::submit`), or the target of a protocol
    /// credit (`Bridge::credit`). Phase-2 will widen the canonical
    /// balance to 256-bit and this variant retires.
    AmountOverflow,
    /// `Bridge::submit` was called with `Intent::Call`. Calls require a
    /// `ContractRegistry`, which only the block executor holds. Lift the
    /// call into a block and use [`crate::BlockExecutor`].
    CallRequiresRegistry,
    /// `Bridge::submit` was called with `Intent::DeployModule`. Module
    /// deploys require a `ModuleStore`, which only the bundle executor
    /// holds. Lift the deploy into a block and use the bundle executor.
    DeployModuleRequiresModuleStore,
    /// A bundle step was a `MoveCall` or `DeployModule` but the bundle
    /// executor was invoked through `execute` (without a Move runtime).
    /// Use `BundleExecutor::execute_with_move_runtime` for bundles
    /// containing Move-VM-bound steps.
    MoveRuntimeRequired,
    /// A `MoveCall` step's executor returned an error (typed via
    /// `MoveExecutionError`). The textual form is in the variant.
    MoveCallFailed(String),
    /// A `DeployModule` step's `ModuleStore::put` rejected the deploy
    /// (already-exists or backend failure). The textual form is in
    /// the variant.
    ModuleDeployFailed(String),
}

/// Wraps a mutable [`State`] reference and offers the only validated path to
/// mutate it. Hold one for the duration of a transaction.
pub struct Bridge<'s> {
    state: &'s mut State,
    token: BridgeToken,
}

impl<'s> Bridge<'s> {
    /// Open a bridge over the given state. Cheap; intended to be created
    /// per-transaction.
    pub fn new(state: &'s mut State) -> Self {
        Self {
            state,
            token: BridgeToken::__for_bridge_only(),
        }
    }

    /// Read-through to state. Lane code uses this for balance lookups so it
    /// never needs to hold a `&State` directly.
    #[must_use]
    pub fn balance_of(&self, addr: &Address) -> Balance {
        self.state.balance_of(addr)
    }

    /// Validate an intent and, if it passes, apply it to state atomically.
    ///
    /// On `Err`, no state mutation occurs.
    ///
    /// # Errors
    ///
    /// Returns [`RejectReason::InsufficientBalance`] when the source balance
    /// is below the requested transfer amount, and
    /// [`RejectReason::AmountOverflow`] when the destination balance would
    /// overflow `u128`.
    #[allow(clippy::needless_pass_by_value)] // by-value communicates "consumed intent"
    pub fn submit(&mut self, intent: Intent) -> Result<(), RejectReason> {
        match intent {
            Intent::Call { .. } => Err(RejectReason::CallRequiresRegistry),
            Intent::DeployModule { .. } => {
                Err(RejectReason::DeployModuleRequiresModuleStore)
            }
            Intent::Transfer { from, to, amount } => {
                let from_balance = self.state.balance_of(&from).0;

                if from_balance < amount {
                    return Err(RejectReason::InsufficientBalance);
                }

                // Self-transfer is a structural no-op. Without this
                // guard the two `apply` calls below race: the second
                // (credit) overwrites the first (debit) and leaves the
                // address at `balance + amount`. We still validate the
                // balance above so the error surface stays consistent
                // with non-self transfers.
                if from == to {
                    return Ok(());
                }

                let to_balance = self.state.balance_of(&to).0;
                let new_to = to_balance
                    .checked_add(amount)
                    .ok_or(RejectReason::AmountOverflow)?;
                let new_from = from_balance - amount;

                self.state.apply(
                    &self.token,
                    &StateChange::SetBalance {
                        addr: from,
                        to: Balance(new_from),
                    },
                );
                self.state.apply(
                    &self.token,
                    &StateChange::SetBalance {
                        addr: to,
                        to: Balance(new_to),
                    },
                );
                Ok(())
            }
        }
    }

    /// Protocol-owned credit: add `amount` to `addr`'s balance with no
    /// debiting counterparty.
    ///
    /// This is the mechanism behind **genesis seeding** and protocol
    /// payouts (inflation, ring rewards, insurance, treasury, slashing
    /// distribution) — cases that [`Bridge::submit`] (transfer-only)
    /// cannot express and that gsx-dag's in-memory substrate handles via a
    /// private `credit_unchecked` path.
    ///
    /// **Authorization is the caller's responsibility.** Like
    /// [`Bridge::submit`], this is pure mechanism: lane separation still
    /// holds (the mutation goes through the [`BridgeToken`] gate), but
    /// whether a credit is legitimate is decided upstream. In gsx-dag only
    /// consensus-authorized, daemon-emitted intents (`GenesisAllocation`,
    /// `MintInflation`, `DistributeRewards`, slashing distribution) reach
    /// this method; user transactions never do.
    ///
    /// **Nonce preserved.** The mutation goes through
    /// [`BalanceSlot::deposit`] on the *existing* slot and writes the
    /// result back via [`StateChange::SetAccount`], not `SetBalance` —
    /// so the target's account nonce survives the payout. A naive
    /// `SetBalance` would zero it, re-enabling replay of every tx the
    /// account previously sent (Codex P1 on the original push of this
    /// PR).
    ///
    /// # Errors
    ///
    /// [`RejectReason::AmountOverflow`] if the resulting balance would
    /// exceed `u128`. A zero `amount` is a no-op.
    pub fn credit(&mut self, addr: Address, amount: Balance) -> Result<(), RejectReason> {
        if amount.0 == 0 {
            return Ok(());
        }
        let mut slot = self.state.slot_of(&addr);
        slot.deposit(amount.0)
            .map_err(|_| RejectReason::AmountOverflow)?;
        self.write_slot(addr, slot);
        Ok(())
    }

    /// Protocol-owned debit: remove `amount` from `addr`'s balance with no
    /// crediting counterparty (slashing burn, escrow / bond drain).
    ///
    /// Same authorization model as [`Bridge::credit`]. The debit goes
    /// through [`BalanceSlot::withdraw`] on the existing slot, so the
    /// account's nonce is preserved (slashing must not silently reset
    /// the victim's transaction counter — that would re-enable replay
    /// of every previously-spent transaction).
    ///
    /// # Errors
    ///
    /// [`RejectReason::InsufficientBalance`] if `addr` holds less than
    /// `amount`. A zero `amount` is a no-op.
    pub fn debit(&mut self, addr: Address, amount: Balance) -> Result<(), RejectReason> {
        if amount.0 == 0 {
            return Ok(());
        }
        let mut slot = self.state.slot_of(&addr);
        slot.withdraw(amount.0)
            .map_err(|_| RejectReason::InsufficientBalance)?;
        self.write_slot(addr, slot);
        Ok(())
    }

    /// Write `slot` for `addr` through the capability gate, preserving
    /// the balance + nonce pair as a unit. Used by the protocol-side
    /// credit / debit paths after mutating a `BalanceSlot` in place
    /// via [`BalanceSlot::deposit`] / [`BalanceSlot::withdraw`]. Funnels
    /// to [`StateChange::SetAccount`] so the nonce travels with the
    /// balance.
    fn write_slot(&mut self, addr: Address, slot: BalanceSlot) {
        self.state.apply(
            &self.token,
            &StateChange::SetAccount {
                addr,
                balance: slot.as_balance(),
                nonce: slot.nonce().value,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::BridgeToken;

    fn seeded_state(addr: Address, amount: u128) -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(amount),
            },
        );
        state
    }

    #[test]
    fn transfer_moves_funds() {
        let alice = Address([1; 20]);
        let bob = Address([2; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge
            .submit(Intent::Transfer {
                from: alice,
                to: bob,
                amount: 30,
            })
            .unwrap();

        assert_eq!(bridge.balance_of(&alice), Balance(70));
        assert_eq!(bridge.balance_of(&bob), Balance(30));
    }

    #[test]
    fn self_transfer_is_a_no_op() {
        // Regression test for a bug found by S4's property test:
        // without the from==to guard, the credit-write of `to`
        // overwrites the debit-write of `from`, inflating the balance.
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge
            .submit(Intent::Transfer {
                from: alice,
                to: alice,
                amount: 30,
            })
            .unwrap();

        assert_eq!(bridge.balance_of(&alice), Balance(100));
    }

    #[test]
    fn self_transfer_still_checks_balance() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 5);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.submit(Intent::Transfer {
            from: alice,
            to: alice,
            amount: 30,
        });

        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(5));
    }

    #[test]
    fn transfer_rejects_insufficient_balance() {
        let alice = Address([1; 20]);
        let bob = Address([2; 20]);
        let mut state = seeded_state(alice, 5);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.submit(Intent::Transfer {
            from: alice,
            to: bob,
            amount: 30,
        });

        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(5));
        assert_eq!(bridge.balance_of(&bob), Balance(0));
    }

    #[test]
    fn deploy_module_routes_to_bundle_executor() {
        // S9.3: Bridge::submit cannot deploy modules — that needs a
        // ModuleStore, which only the bundle executor owns. Until
        // S9.4 wires the bundle executor, surface this as a typed
        // rejection so callers learn to use the bundle path.
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        let mut account_bytes = [0u8; 32];
        account_bytes[12..32].copy_from_slice(&alice.0);
        let result = bridge.submit(Intent::DeployModule {
            account: gsxdb_state::MoveAddress(account_bytes),
            name: gsxdb_state::Identifier::new("hello_module").unwrap(),
            bytes: vec![0xCA, 0xFE, 0xBA, 0xBE],
        });

        assert_eq!(result, Err(RejectReason::DeployModuleRequiresModuleStore));
        // No state mutation.
        assert_eq!(bridge.balance_of(&alice), Balance(100));
    }

    #[test]
    fn credit_seeds_a_fresh_address() {
        // Genesis-seeding path: credit from zero with no counterparty —
        // the case `submit` (transfer-only) cannot express.
        let alice = Address([1; 20]);
        let mut state = State::default();

        let mut bridge = Bridge::new(&mut state);
        bridge.credit(alice, Balance(1_000)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_000));

        // Credits accumulate.
        bridge.credit(alice, Balance(500)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_500));

        // Zero is a no-op.
        bridge.credit(alice, Balance(0)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_500));
    }

    #[test]
    fn credit_rejects_overflow() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, u128::MAX);

        let mut bridge = Bridge::new(&mut state);
        let result = bridge.credit(alice, Balance(1));
        assert_eq!(result, Err(RejectReason::AmountOverflow));
        // Balance unchanged on rejection.
        assert_eq!(bridge.balance_of(&alice), Balance(u128::MAX));
    }

    #[test]
    fn debit_reduces_balance_and_guards_underflow() {
        let alice = Address([1; 20]);
        let mut state = seeded_state(alice, 100);

        let mut bridge = Bridge::new(&mut state);
        bridge.debit(alice, Balance(30)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(70));

        // Debiting more than held rejects and leaves the balance intact.
        let result = bridge.debit(alice, Balance(1_000));
        assert_eq!(result, Err(RejectReason::InsufficientBalance));
        assert_eq!(bridge.balance_of(&alice), Balance(70));

        // Zero is a no-op.
        bridge.debit(alice, Balance(0)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(70));
    }

    /// Regression for the Codex-style finding that `credit` / `debit`
    /// previously routed through `SetBalance`, which `State::apply`
    /// implements via `BalanceSlot::new(...)` — resetting the slot's
    /// nonce to zero. A protocol payout (inflation, ring reward) or
    /// slashing-burn against an already-active EOA would silently
    /// re-enable replay of every transaction the account had
    /// previously sent. The new code goes through
    /// `BalanceSlot::deposit` / `withdraw` on the existing slot and
    /// writes back via `SetAccount`, so the nonce travels with the
    /// balance.
    #[test]
    fn credit_and_debit_preserve_account_nonce() {
        use gsxdb_state::AccountNonce;
        let alice = Address([1; 20]);
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();

        // Pre-seed alice at balance 1000, nonce 42 (e.g. from prior EVM
        // activity). Using SetAccount directly so the test setup
        // doesn't depend on the credit path it's exercising.
        state.apply(
            &token,
            &StateChange::SetAccount {
                addr: alice,
                balance: Balance(1_000),
                nonce: 42,
            },
        );
        assert_eq!(state.slot_of(&alice).nonce(), AccountNonce::new(42));

        // Credit must NOT zero the nonce.
        let mut bridge = Bridge::new(&mut state);
        bridge.credit(alice, Balance(500)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_500));
        assert_eq!(
            state.slot_of(&alice).nonce(),
            AccountNonce::new(42),
            "credit must preserve the target's account nonce"
        );

        // Debit on the same account preserves nonce too.
        let mut bridge = Bridge::new(&mut state);
        bridge.debit(alice, Balance(200)).unwrap();
        assert_eq!(bridge.balance_of(&alice), Balance(1_300));
        assert_eq!(
            state.slot_of(&alice).nonce(),
            AccountNonce::new(42),
            "debit must preserve the target's account nonce"
        );

        // Rejected credit (overflow) must also leave nonce intact.
        let huge = u128::MAX - 100; // would-be balance > u128::MAX
        let mut bridge = Bridge::new(&mut state);
        let err = bridge.credit(alice, Balance(huge)).unwrap_err();
        assert_eq!(err, RejectReason::AmountOverflow);
        assert_eq!(bridge.balance_of(&alice), Balance(1_300));
        assert_eq!(state.slot_of(&alice).nonce(), AccountNonce::new(42));

        // Rejected debit (underflow) — same.
        let mut bridge = Bridge::new(&mut state);
        let err = bridge.debit(alice, Balance(1_000_000)).unwrap_err();
        assert_eq!(err, RejectReason::InsufficientBalance);
        assert_eq!(bridge.balance_of(&alice), Balance(1_300));
        assert_eq!(state.slot_of(&alice).nonce(), AccountNonce::new(42));
    }
}
