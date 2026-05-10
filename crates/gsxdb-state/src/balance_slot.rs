//! `BalanceSlot` — single canonical balance + nonce with two VM-shaped projections.
//!
//! Proposition 1 says `EVM (balance, nonce) == Move (Coin.value, sequence)` at every
//! checkpoint. This is enforced structurally: one canonical balance field, one canonical
//! nonce field, with both projections deriving from them. No call sequence can desynchronise them.
//!
//! Phase-1 (S1-S8) used `u128` for balance only. Phase-2 (S9+) adds nonce (u64) as a
//! dual-projection component. EVM `uint256` values exceeding `u128::MAX` remain
//! unrepresentable; S10 will lift to 256-bit.

use crate::nonce_semantics::AccountNonce;
use crate::Balance;

/// Balance projected for the EVM. Carries the same numeric value as the
/// canonical slot but is a distinct type to keep accidental mixing of EVM
/// and Move balances from compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EvmBalance(u128);

impl EvmBalance {
    /// Underlying numeric value.
    #[must_use]
    pub fn to_u128(self) -> u128 {
        self.0
    }
}

/// Balance projected for the Move VM. See [`EvmBalance`] for why this is a
/// distinct type from the canonical [`Balance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveCoinValue(u128);

impl MoveCoinValue {
    /// Underlying numeric value.
    #[must_use]
    pub fn to_u128(self) -> u128 {
        self.0
    }
}

/// Errors that arise when mutating a balance slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotError {
    /// Deposit would overflow `u128`. Phase-2 widens to 256-bit.
    Overflow,
    /// Withdraw exceeds the current balance.
    Underflow,
}

/// A canonical balance + nonce with two VM-shaped projections.
///
/// The extended dual-projection invariant (S9+) — that the EVM and Move views
/// always agree on both balance and nonce — is structural: there are two source
/// fields (balance, nonce), and both projections derive from them. No call
/// sequence can desynchronise them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BalanceSlot {
    canonical: u128,
    nonce: AccountNonce,
}

impl Default for BalanceSlot {
    fn default() -> Self {
        Self {
            canonical: 0,
            nonce: AccountNonce::default(),
        }
    }
}

impl BalanceSlot {
    /// New slot with the given canonical balance value and zero nonce.
    #[must_use]
    pub fn new(value: u128) -> Self {
        Self {
            canonical: value,
            nonce: AccountNonce::default(),
        }
    }

    /// New slot with the given balance and nonce.
    #[must_use]
    pub fn with_nonce(value: u128, nonce: AccountNonce) -> Self {
        Self {
            canonical: value,
            nonce,
        }
    }

    /// Canonical numeric balance value. Equivalent to either projection's `to_u128`.
    #[must_use]
    pub fn canonical(self) -> u128 {
        self.canonical
    }

    /// Canonical nonce. Shared between EVM and Move projections.
    #[must_use]
    pub fn nonce(self) -> AccountNonce {
        self.nonce
    }

    /// EVM-shaped projection of balance.
    #[must_use]
    pub fn evm_balance(self) -> EvmBalance {
        EvmBalance(self.canonical)
    }

    /// Move-shaped projection of balance (same numeric value as EVM).
    #[must_use]
    pub fn move_coin_value(self) -> MoveCoinValue {
        MoveCoinValue(self.canonical)
    }

    /// Convert balance to the storage-side [`Balance`] newtype.
    #[must_use]
    pub fn as_balance(self) -> Balance {
        Balance(self.canonical)
    }

    /// Add `amount` to the canonical balance.
    ///
    /// # Errors
    ///
    /// Returns [`SlotError::Overflow`] if the result would exceed `u128::MAX`.
    pub fn deposit(&mut self, amount: u128) -> Result<(), SlotError> {
        self.canonical = self
            .canonical
            .checked_add(amount)
            .ok_or(SlotError::Overflow)?;
        Ok(())
    }

    /// Subtract `amount` from the canonical balance.
    ///
    /// # Errors
    ///
    /// Returns [`SlotError::Underflow`] if `amount` exceeds the current balance.
    pub fn withdraw(&mut self, amount: u128) -> Result<(), SlotError> {
        self.canonical = self
            .canonical
            .checked_sub(amount)
            .ok_or(SlotError::Underflow)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projections_agree_on_default() {
        let slot = BalanceSlot::default();
        assert_eq!(
            slot.evm_balance().to_u128(),
            slot.move_coin_value().to_u128()
        );
        assert_eq!(slot.nonce().value, 0);
    }

    #[test]
    fn balance_and_nonce_both_preserved() {
        let nonce = super::super::nonce_semantics::AccountNonce::new(42);
        let slot = BalanceSlot::with_nonce(1000, nonce);
        assert_eq!(slot.canonical(), 1000);
        assert_eq!(slot.nonce().value, 42);
    }

    #[test]
    fn deposit_then_withdraw_round_trips() {
        let mut slot = BalanceSlot::new(100);
        slot.deposit(25).unwrap();
        assert_eq!(slot.canonical(), 125);
        slot.withdraw(50).unwrap();
        assert_eq!(slot.canonical(), 75);
    }

    #[test]
    fn deposit_overflow_is_rejected() {
        let mut slot = BalanceSlot::new(u128::MAX);
        assert_eq!(slot.deposit(1), Err(SlotError::Overflow));
        // Slot value unchanged on error.
        assert_eq!(slot.canonical(), u128::MAX);
    }

    #[test]
    fn withdraw_underflow_is_rejected() {
        let mut slot = BalanceSlot::new(10);
        assert_eq!(slot.withdraw(11), Err(SlotError::Underflow));
        // Slot value unchanged on error.
        assert_eq!(slot.canonical(), 10);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    /// One legal mutation that can be applied to a `BalanceSlot`.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Deposit(u128),
        Withdraw(u128),
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            any::<u128>().prop_map(Op::Deposit),
            any::<u128>().prop_map(Op::Withdraw),
        ]
    }

    proptest! {
        /// **Dual-projection invariant** (Proposition 1, structural form):
        /// after any sequence of legal or rejected mutations, the EVM and
        /// Move projections of a `BalanceSlot` agree on the numeric value.
        ///
        /// This is the load-bearing test for S2. It must pass for the sprint
        /// to close.
        #[test]
        fn projections_always_agree(
            initial in any::<u128>(),
            ops in proptest::collection::vec(op_strategy(), 0..64),
        ) {
            let mut slot = BalanceSlot::new(initial);

            // Check after construction.
            prop_assert_eq!(
                slot.evm_balance().to_u128(),
                slot.move_coin_value().to_u128()
            );

            for op in ops {
                // Errors are fine — the invariant must hold whether the op
                // succeeded or was rejected.
                let _ = match op {
                    Op::Deposit(n) => slot.deposit(n),
                    Op::Withdraw(n) => slot.withdraw(n),
                };
                prop_assert_eq!(
                    slot.evm_balance().to_u128(),
                    slot.move_coin_value().to_u128(),
                );
                // Also: both projections must equal the canonical value.
                prop_assert_eq!(slot.evm_balance().to_u128(), slot.canonical());
                prop_assert_eq!(slot.move_coin_value().to_u128(), slot.canonical());
            }
        }

        /// Successful deposits never decrease the canonical balance, and
        /// successful withdrawals never increase it.
        #[test]
        fn mutations_move_in_the_right_direction(
            initial in any::<u128>(),
            amount in any::<u128>(),
        ) {
            let mut slot = BalanceSlot::new(initial);
            let before = slot.canonical();

            if slot.deposit(amount).is_ok() {
                prop_assert!(slot.canonical() >= before);
            }

            let mut slot = BalanceSlot::new(initial);
            let before = slot.canonical();
            if slot.withdraw(amount).is_ok() {
                prop_assert!(slot.canonical() <= before);
            }
        }

        /// Rejected mutations leave the canonical value unchanged.
        #[test]
        fn rejected_mutations_are_atomic(
            initial in any::<u128>(),
            amount in any::<u128>(),
        ) {
            let mut slot = BalanceSlot::new(initial);
            let before = slot.canonical();
            if slot.deposit(amount).is_err() {
                prop_assert_eq!(slot.canonical(), before);
            }

            let mut slot = BalanceSlot::new(initial);
            let before = slot.canonical();
            if slot.withdraw(amount).is_err() {
                prop_assert_eq!(slot.canonical(), before);
            }
        }
    }
}
