//! VM-shape mock executors.
//!
//! Both [`MockEvm`] and [`MockMove`] route through
//! [`crate::Bridge::submit`]. They never touch [`State`] directly. This
//! is what gives the executors their shared invariant: any two VM-shape
//! transactions that canonicalise to the same [`Intent`] produce
//! identical state, because they pass through the same single mutation
//! path.
//!
//! Error semantics are VM-flavoured:
//!
//! - [`EvmError`] mirrors EVM revert: any error reverts the whole tx,
//!   leaving state untouched.
//! - [`MoveError`] mirrors Move abort: any error aborts the txn,
//!   leaving state untouched.
//!
//! Both collapse to "no state change on error" through the bridge,
//! which matches both VMs' expectations.

use crate::{Bridge, Intent, RejectReason};
use gsxdb_state::{EvmTx, MoveTx, State};

/// EVM-flavoured error from [`MockEvm::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvmError {
    /// `from` had less than `value` available. EVM would revert.
    Revert(RejectReason),
}

/// Move-flavoured error from [`MockMove::execute`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveError {
    /// The signer's `Coin` had less than `amount`. Move would abort.
    Abort(RejectReason),
}

impl From<RejectReason> for EvmError {
    fn from(r: RejectReason) -> Self {
        EvmError::Revert(r)
    }
}

impl From<RejectReason> for MoveError {
    fn from(r: RejectReason) -> Self {
        MoveError::Abort(r)
    }
}

/// EVM mock executor.
///
/// Models EVM transfer semantics: validate balance, debit source,
/// credit destination, atomic-on-error. Real revm integration lands per
/// IQ-2; the property tests guarantee whatever replaces this preserves
/// the dual-projection invariant.
#[derive(Debug, Default, Clone, Copy)]
pub struct MockEvm;

impl MockEvm {
    /// Execute an EVM-shape transfer against `state`.
    ///
    /// # Errors
    ///
    /// Returns [`EvmError::Revert`] when the bridge rejects the
    /// canonical intent (insufficient balance, overflow). State is
    /// untouched on error.
    pub fn execute(self, state: &mut State, tx: EvmTx) -> Result<(), EvmError> {
        let canonical = tx.to_canonical();
        let mut bridge = Bridge::new(state);
        bridge.submit(Intent::Transfer {
            from: canonical.from,
            to: canonical.to,
            amount: canonical.amount,
        })?;
        Ok(())
    }
}

/// Move mock executor.
///
/// Models Move transfer semantics on a `Coin<T>`-flavoured resource:
/// validate balance, withdraw from signer, deposit to recipient,
/// atomic-on-abort. Real Move VM integration is blocked on choosing a
/// Move VM crate; see IQ-2.
#[derive(Debug, Default, Clone, Copy)]
pub struct MockMove;

impl MockMove {
    /// Execute a Move-shape transfer against `state`.
    ///
    /// # Errors
    ///
    /// Returns [`MoveError::Abort`] when the bridge rejects the
    /// canonical intent. State is untouched on error.
    pub fn execute(self, state: &mut State, tx: MoveTx) -> Result<(), MoveError> {
        let canonical = tx.to_canonical();
        let mut bridge = Bridge::new(state);
        bridge.submit(Intent::Transfer {
            from: canonical.from,
            to: canonical.to,
            amount: canonical.amount,
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::{Address, Balance, BridgeToken, EvmView, MoveView, StateChange};
    use gsxdb_state::{EvmProjector, MoveProjector};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn seeded(addr_in: Address, amount: u128) -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr_in,
                to: Balance(amount),
            },
        );
        state
    }

    #[test]
    fn evm_execute_moves_funds() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 100);

        MockEvm
            .execute(
                &mut state,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 30,
                    nonce: 0,
                },
            )
            .unwrap();

        assert_eq!(state.balance_of(&alice), Balance(70));
        assert_eq!(state.balance_of(&bob), Balance(30));
    }

    #[test]
    fn move_execute_moves_funds() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 100);

        MockMove
            .execute(
                &mut state,
                MoveTx {
                    signer: alice,
                    recipient: bob,
                    amount: 30,
                },
            )
            .unwrap();

        assert_eq!(state.balance_of(&alice), Balance(70));
        assert_eq!(state.balance_of(&bob), Balance(30));
    }

    #[test]
    fn evm_revert_leaves_state_untouched() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 5);

        let err = MockEvm
            .execute(
                &mut state,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 30,
                    nonce: 0,
                },
            )
            .unwrap_err();

        assert_eq!(err, EvmError::Revert(RejectReason::InsufficientBalance));
        assert_eq!(state.balance_of(&alice), Balance(5));
        assert_eq!(state.balance_of(&bob), Balance(0));
    }

    #[test]
    fn move_abort_leaves_state_untouched() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 5);

        let err = MockMove
            .execute(
                &mut state,
                MoveTx {
                    signer: alice,
                    recipient: bob,
                    amount: 30,
                },
            )
            .unwrap_err();

        assert_eq!(err, MoveError::Abort(RejectReason::InsufficientBalance));
        assert_eq!(state.balance_of(&alice), Balance(5));
        assert_eq!(state.balance_of(&bob), Balance(0));
    }

    #[test]
    fn evm_and_move_canonical_equivalents_produce_identical_state() {
        // Same logical operation expressed in both VM shapes against
        // independent states. End-state must be identical.
        let alice = addr(1);
        let bob = addr(2);

        let mut s_evm = seeded(alice, 100);
        MockEvm
            .execute(
                &mut s_evm,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 25,
                    nonce: 0,
                },
            )
            .unwrap();

        let mut s_move = seeded(alice, 100);
        MockMove
            .execute(
                &mut s_move,
                MoveTx {
                    signer: alice,
                    recipient: bob,
                    amount: 25,
                },
            )
            .unwrap();

        // Both projections agree on each side, and the two sides agree
        // with each other.
        assert_eq!(
            EvmView.balance_of(&s_evm, &alice),
            EvmView.balance_of(&s_move, &alice)
        );
        assert_eq!(
            MoveView.coin_value(&s_evm, &bob),
            MoveView.coin_value(&s_move, &bob)
        );
    }
}
