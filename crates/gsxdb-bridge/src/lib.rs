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

pub mod anchor;
pub mod bundle;
pub mod occ;
pub mod recovery;
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
pub use vm::{EvmError, MockEvm, MockMove, MoveError};

use gsxdb_state::{Address, Balance, BridgeToken, State, StateChange};

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
}

/// Reasons an intent can be rejected during validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RejectReason {
    /// Source balance is below the requested transfer amount.
    InsufficientBalance,
    /// Transfer amount overflowed `u128` arithmetic. Phase-2 will use a
    /// 256-bit type internally.
    AmountOverflow,
    /// `Bridge::submit` was called with `Intent::Call`. Calls require a
    /// `ContractRegistry`, which only the block executor holds. Lift the
    /// call into a block and use [`crate::BlockExecutor`].
    CallRequiresRegistry,
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
}
