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

use crate::{Address, BalanceSlot, EvmBalance, MoveCoinValue, State};

/// Read EVM-shaped state.
pub trait EvmProjector {
    /// EVM-shaped balance read for `addr`. Equivalent to evaluating
    /// `addr.balance` (an externally-owned account) or `balanceOf(addr)`
    /// (an ERC-20-like contract that aliases to the canonical balance)
    /// in EVM bytecode.
    fn balance_of(&self, state: &State, addr: &Address) -> EvmBalance;

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
}
