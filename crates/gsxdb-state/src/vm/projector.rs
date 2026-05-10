//! Read-side VM projectors.
//!
//! Reads from a [`State`] in the shape each VM expects:
//!
//! - [`EvmProjector::balance_of`] returns an [`EvmBalance`] — the EVM's
//!   `address.balance` view.
//! - [`MoveProjector::coin_value`] returns a [`MoveCoinValue`] — the
//!   Move `Coin::value` view.
//!
//! Both delegate to [`State::slot_of`] and project the canonical
//! [`BalanceSlot`]; that's what makes them structurally consistent.
//! The dual-projection invariant (Proposition 1) is true at this layer
//! by construction — there is one canonical field, and both projections
//! read from it.
//!
//! Phase 2 (S9+) extends the invariant to nonce: one canonical nonce field
//! projects to both EVM nonce and Move sequence number.

use crate::{
    Address, BalanceSlot, EvmBalance, EvmNonce, MoveCoinValue, MoveSequenceNumber,
    State,
};

/// Read EVM-shaped state.
pub trait EvmProjector {
    /// EVM-shaped balance read for `addr`. Equivalent to evaluating
    /// `addr.balance` (an externally-owned account) or `balanceOf(addr)`
    /// (an ERC-20-like contract that aliases to the canonical balance)
    /// in EVM bytecode.
    fn balance_of(&self, state: &State, addr: &Address) -> EvmBalance;

    /// EVM nonce read for `addr`. Equivalent to evaluating `nonce[addr]`
    /// in EVM bytecode. This nonce is incremented on every transaction
    /// originated by the account.
    fn nonce(&self, state: &State, addr: &Address) -> EvmNonce {
        EvmNonce(state.slot_of(addr).nonce().evm_nonce().0)
    }

    /// Read the full canonical slot. Most callers want
    /// [`Self::balance_of`]; this is exposed for property tests that
    /// compare projections side by side.
    fn slot(&self, state: &State, addr: &Address) -> BalanceSlot {
        state.slot_of(addr)
    }
}

/// Read Move-shaped state.
pub trait MoveProjector {
    /// Move-shaped balance read for `addr`. Equivalent to evaluating
    /// `Coin::value(&Coin)` against the resource at `addr` in Move
    /// bytecode.
    fn coin_value(&self, state: &State, addr: &Address) -> MoveCoinValue;

    /// Move sequence number read for `addr`. Equivalent to evaluating
    /// `account.sequence_number` in Move bytecode. This number increments
    /// on every transaction from the account (same semantics as EVM nonce).
    fn sequence_number(&self, state: &State, addr: &Address) -> MoveSequenceNumber {
        MoveSequenceNumber(state.slot_of(addr).nonce().move_sequence().0)
    }

    /// Read the full canonical slot. See [`EvmProjector::slot`].
    fn slot(&self, state: &State, addr: &Address) -> BalanceSlot {
        state.slot_of(addr)
    }
}

/// Default EVM projector. Reads via [`State::slot_of`] and returns the
/// EVM projection of the canonical slot.
#[derive(Debug, Default, Clone, Copy)]
pub struct EvmView;

impl EvmProjector for EvmView {
    fn balance_of(&self, state: &State, addr: &Address) -> EvmBalance {
        state.slot_of(addr).evm_balance()
    }
}

/// Default Move projector. Reads via [`State::slot_of`] and returns the
/// Move projection of the canonical slot.
#[derive(Debug, Default, Clone, Copy)]
pub struct MoveView;

impl MoveProjector for MoveView {
    fn coin_value(&self, state: &State, addr: &Address) -> MoveCoinValue {
        state.slot_of(addr).move_coin_value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Balance, BridgeToken, StateChange};

    fn seeded(addr: Address, amount: u128) -> State {
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
    fn empty_state_reads_zero_through_both_projections() {
        let state = State::default();
        let addr = Address([1; 20]);
        assert_eq!(EvmView.balance_of(&state, &addr).to_u128(), 0);
        assert_eq!(MoveView.coin_value(&state, &addr).to_u128(), 0);
    }

    #[test]
    fn populated_state_reads_canonical_through_both() {
        let addr = Address([1; 20]);
        let state = seeded(addr, 42);

        assert_eq!(EvmView.balance_of(&state, &addr).to_u128(), 42);
        assert_eq!(MoveView.coin_value(&state, &addr).to_u128(), 42);
    }

    #[test]
    fn projections_agree_at_extremes() {
        let addr = Address([1; 20]);
        for value in [0u128, 1, u128::MAX / 2, u128::MAX - 1, u128::MAX] {
            let state = seeded(addr, value);
            let evm = EvmView.balance_of(&state, &addr).to_u128();
            let mv = MoveView.coin_value(&state, &addr).to_u128();
            assert_eq!(evm, mv, "disagreement at value={value}");
            assert_eq!(evm, value);
        }
    }

    #[test]
    fn slot_accessor_matches_state_slot_of() {
        let addr = Address([1; 20]);
        let state = seeded(addr, 100);

        let direct = state.slot_of(&addr);
        let via_evm = EvmProjector::slot(&EvmView, &state, &addr);
        let via_move = MoveProjector::slot(&MoveView, &state, &addr);

        assert_eq!(direct, via_evm);
        assert_eq!(direct, via_move);
    }

    #[test]
    fn nonce_projections_agree() {
        let state = State::default();
        let addr = Address([1; 20]);

        // Both projections should read nonce 0 by default.
        let evm_nonce = EvmView.nonce(&state, &addr);
        let move_seq = MoveView.sequence_number(&state, &addr);

        assert_eq!(evm_nonce.0, 0);
        assert_eq!(move_seq.0, 0);
        assert_eq!(evm_nonce.0, move_seq.0);
    }

    #[test]
    fn nonce_dual_projection_invariant() {
        use crate::AccountNonce;

        let mut state = State::default();
        let token = crate::BridgeToken::__for_bridge_only();
        let addr = Address([5; 20]);

        // Set balance.
        state.apply(
            &token,
            &crate::StateChange::SetBalance {
                addr,
                to: crate::Balance(100),
            },
        );

        // Note: StateChange::SetBalance currently resets nonce to default.
        // This test documents that constraint; nonce mutation via StateChange
        // would require a new variant or extended SetBalance.
        let evm_nonce = EvmView.nonce(&state, &addr);
        let move_seq = MoveView.sequence_number(&state, &addr);

        // Both should be 0 (default nonce after SetBalance).
        assert_eq!(evm_nonce.0, 0);
        assert_eq!(move_seq.0, 0);
        assert_eq!(evm_nonce.0, move_seq.0);

        // Document the with_nonce constructor works structurally even if
        // StateChange doesn't yet support nonce mutation.
        let _ = crate::BalanceSlot::with_nonce(100, AccountNonce::new(42));
    }
}
