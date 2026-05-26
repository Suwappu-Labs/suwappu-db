//! Real EVM executor backed by `gsx-revm` (monad-revm / bluealloy revm 34).
//!
//! Replaces [`MockEvm`](crate::vm::MockEvm) — which only shuffled balances
//! through `Bridge::submit` — with real EVM execution: the transaction runs
//! on the Monad EVM (`MonadNine` spec, `0x1000` staking precompile), charges
//! real gas, advances the sender nonce, and the resulting state diff is
//! written back through the `BridgeToken` gate.
//!
//! Scope (increment 1): value transfers over the canonical balance map.
//! `EvmTx` carries no calldata, and gsx-db has no contract code/storage
//! store yet, so the [`DatabaseRef`] adapter reports empty code/storage.
//! Full contract execution (a code + storage store outside the
//! dual-projection `BalanceSlot`) is a follow-on increment.

use core::convert::Infallible;

use gsx_revm::{monad_context_with_db, MonadBuilder};
use revm::{
    bytecode::Bytecode,
    context::TxEnv,
    database::WrapDatabaseRef,
    database_interface::DatabaseRef,
    primitives::{Address as RevmAddress, Bytes, B256, KECCAK_EMPTY, U256},
    state::AccountInfo,
    ExecuteEvm,
};

use gsxdb_state::{Address, Balance, EvmTx, State};

use crate::{vm::EvmError, Bridge, RejectReason};

/// Default gas limit for a value transfer (the EVM intrinsic cost).
const TRANSFER_GAS_LIMIT: u64 = 21_000;

/// Read-only [`DatabaseRef`] view of gsx-db [`State`] for revm.
///
/// Backs EVM account *basics* (balance + nonce) from the canonical
/// `BalanceSlot`. Code and storage are empty: increment 1 covers value
/// transfers only; a contract code/storage store is a follow-on. Reads are
/// infallible — an unseen address reads as a zero-balance, zero-nonce
/// account.
struct GsxStateDb<'a> {
    state: &'a State,
}

impl DatabaseRef for GsxStateDb<'_> {
    type Error = Infallible;

    fn basic_ref(&self, address: RevmAddress) -> Result<Option<AccountInfo>, Self::Error> {
        let slot = self.state.slot_of(&Address(address.into_array()));
        Ok(Some(AccountInfo {
            balance: U256::from(slot.canonical()),
            nonce: slot.nonce().value,
            code_hash: KECCAK_EMPTY,
            ..AccountInfo::default()
        }))
    }

    fn code_by_hash_ref(&self, _code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(Bytecode::default())
    }

    fn storage_ref(&self, _address: RevmAddress, _index: U256) -> Result<U256, Self::Error> {
        Ok(U256::ZERO)
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

/// Real EVM executor (monad-revm). Drop-in for [`MockEvm`](crate::vm::MockEvm).
#[derive(Debug, Default, Clone, Copy)]
pub struct RevmExecutor;

impl RevmExecutor {
    /// Execute an EVM-shape transfer against `state` through real revm.
    ///
    /// # Errors
    ///
    /// Returns [`EvmError::Revert`] if revm rejects the transaction (e.g.
    /// the sender cannot cover the value) or it reverts/halts. State is
    /// untouched on error.
    pub fn execute(self, state: &mut State, tx: EvmTx) -> Result<(), EvmError> {
        // The sender's current nonce — revm validates `tx.nonce == account.nonce`.
        let sender_nonce = state.slot_of(&tx.from).nonce().value;

        // Run revm over a read-only view, capturing the owned result + state
        // diff so the immutable borrow ends before we re-borrow mutably to
        // write back.
        let (result, diff) = {
            let db = GsxStateDb { state: &*state };
            let mut evm = monad_context_with_db(WrapDatabaseRef(db)).build_monad();
            let txenv = TxEnv::builder()
                .caller(RevmAddress::from(tx.from.0))
                .to(RevmAddress::from(tx.to.0))
                .value(U256::from(tx.value))
                .gas_limit(TRANSFER_GAS_LIMIT)
                .gas_price(0)
                .nonce(sender_nonce)
                .data(Bytes::new())
                .build_fill();
            let out = evm
                .transact(txenv)
                .map_err(|_| EvmError::Revert(RejectReason::InsufficientBalance))?;
            (out.result, out.state)
        };

        if !result.is_success() {
            return Err(EvmError::Revert(RejectReason::InsufficientBalance));
        }

        // Write the post-execution diff (balance + nonce) back through the
        // capability gate, for every touched account.
        let mut bridge = Bridge::new(state);
        for (addr, account) in diff {
            if !account.is_touched() {
                continue;
            }
            let balance = u128::try_from(account.info.balance).unwrap_or(u128::MAX);
            bridge.set_account(Address(addr.into_array()), Balance(balance), account.info.nonce);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::{BridgeToken, StateChange};

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
    fn real_evm_transfer_moves_funds_and_bumps_nonce() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 100);

        RevmExecutor
            .execute(
                &mut state,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 30,
                },
            )
            .unwrap();

        // Balance effect matches a plain transfer (gas_price 0 → no fee).
        assert_eq!(state.balance_of(&alice), Balance(70));
        assert_eq!(state.balance_of(&bob), Balance(30));
        // Real revm advanced the sender's nonce (the mock never did).
        assert_eq!(state.slot_of(&alice).nonce().value, 1);
    }

    #[test]
    fn real_evm_insufficient_balance_reverts_untouched() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 5);

        let err = RevmExecutor
            .execute(
                &mut state,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 30,
                },
            )
            .unwrap_err();

        assert_eq!(err, EvmError::Revert(RejectReason::InsufficientBalance));
        assert_eq!(state.balance_of(&alice), Balance(5));
        assert_eq!(state.balance_of(&bob), Balance(0));
        // Untouched: nonce did not advance on a rejected tx.
        assert_eq!(state.slot_of(&alice).nonce().value, 0);
    }
}
