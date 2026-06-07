//! Real EVM executor backed by `gsx-revm` (monad-revm / bluealloy revm 34).
//!
//! Replaces [`MockEvm`](crate::vm::MockEvm) with real EVM execution on the
//! Monad EVM (`MonadNine` spec, `0x1000` staking precompile): transactions
//! run real bytecode, charge gas, advance the sender nonce, and the
//! resulting state diff is written back through the `BridgeToken` gate.
//!
//! Supports value transfers ([`RevmExecutor::execute`]) and contract calls
//! ([`RevmExecutor::execute_call`]). Account basics (balance + nonce) come
//! from the canonical `BalanceSlot`; contract code and storage come from
//! suwappu-db's EVM-only `evm_code` / `evm_storage` stores.
//!
//! **Not yet consensus-safe for contracts:** EVM code + storage are not yet
//! folded into the verkle state root, so contract state is not committed
//! across validators. That is the consensus-critical follow-on; until then
//! this is real in-process execution for development + validation.

use core::convert::Infallible;

use gsx_revm::{monad_context_with_db, MonadBuilder};
use revm::{
    bytecode::Bytecode,
    context::TxEnv,
    database::WrapDatabaseRef,
    database_interface::DatabaseRef,
    primitives::{Address as RevmAddress, Bytes, TxKind, B256, KECCAK_EMPTY, U256},
    state::{Account, AccountInfo, EvmState},
    ExecuteEvm,
};

use suwappudb_state::{Address, Balance, EvmTx, State};

use crate::{vm::EvmError, Bridge, RejectReason};

/// Gas limit for a bare value transfer (the EVM intrinsic cost).
const TRANSFER_GAS_LIMIT: u64 = 21_000;
/// Gas limit for a contract call. Generous block-scale ceiling; the bounded
/// budget exists to halt runaway execution, not to price precisely (gas is
/// free here — `gas_price` is 0).
const CALL_GAS_LIMIT: u64 = 30_000_000;

/// Read-only [`DatabaseRef`] view of suwappu-db [`State`] for revm.
struct SuwappuStateDb<'a> {
    state: &'a State,
}

impl SuwappuStateDb<'_> {
    /// Build revm's account view for `suwappu`, inlining contract code so revm
    /// rarely needs `code_by_hash`.
    fn account_info(&self, suwappu: &Address) -> AccountInfo {
        let slot = self.state.slot_of(suwappu);
        let (code_hash, code) = match self.state.account_code_hash(suwappu) {
            Some(h) => {
                let code = self
                    .state
                    .code_by_hash(&h)
                    .map(|c| Bytecode::new_raw(Bytes::copy_from_slice(c)));
                (B256::from(h), code)
            }
            None => (KECCAK_EMPTY, None),
        };
        AccountInfo {
            balance: U256::from(slot.canonical()),
            nonce: slot.nonce().value,
            code_hash,
            code,
            ..AccountInfo::default()
        }
    }
}

impl DatabaseRef for SuwappuStateDb<'_> {
    type Error = Infallible;

    fn basic_ref(&self, address: RevmAddress) -> Result<Option<AccountInfo>, Self::Error> {
        let suwappu = Address(address.into_array());
        let slot = self.state.slot_of(&suwappu);
        // Distinguish a never-written account from an existing empty one:
        // an address with no balance, no nonce, and no code does not exist,
        // so opcodes like EXTCODEHASH / account-existence checks branch
        // correctly instead of seeing a phantom KECCAK_EMPTY account.
        if slot.canonical() == 0
            && slot.nonce().value == 0
            && self.state.account_code_hash(&suwappu).is_none()
        {
            return Ok(None);
        }
        Ok(Some(self.account_info(&suwappu)))
    }

    fn code_by_hash_ref(&self, code_hash: B256) -> Result<Bytecode, Self::Error> {
        Ok(self
            .state
            .code_by_hash(&code_hash.0)
            .map(|c| Bytecode::new_raw(Bytes::copy_from_slice(c)))
            .unwrap_or_default())
    }

    fn storage_ref(&self, address: RevmAddress, index: U256) -> Result<U256, Self::Error> {
        let slot = index.to_be_bytes::<32>();
        let value = self
            .state
            .storage_at(&Address(address.into_array()), &slot);
        Ok(U256::from_be_bytes(value))
    }

    fn block_hash_ref(&self, _number: u64) -> Result<B256, Self::Error> {
        Ok(B256::ZERO)
    }
}

/// Write a revm post-execution state diff back through the capability gate:
/// balance + nonce for every touched account, contract code for created
/// accounts, and every changed storage slot.
fn write_back(state: &mut State, diff: EvmState) -> Result<(), EvmError> {
    // Pre-flight: collect the touched accounts, converting every balance to
    // u128 first. A balance above u128::MAX reverts the whole tx (atomic)
    // rather than saturating to u128::MAX — saturating would debit a sender
    // without fully crediting a recipient, and a partial write-back would
    // leave inconsistent state. The refs borrow `diff`, which outlives this.
    let mut writes: Vec<(Address, u128, &Account)> = Vec::new();
    for (addr, account) in &diff {
        if !account.is_touched() {
            continue;
        }
        let balance = u128::try_from(account.info.balance)
            .map_err(|_| EvmError::Revert(RejectReason::BalanceOverflow))?;
        writes.push((Address(addr.into_array()), balance, account));
    }

    let mut bridge = Bridge::new(state);
    for (suwappu, balance, account) in writes {
        bridge.set_account(suwappu, Balance(balance), account.info.nonce);

        if account.is_created() {
            if let Some(code) = &account.info.code {
                if !code.is_empty() {
                    bridge.set_code(account.info.code_hash.0, code.original_bytes().to_vec());
                    bridge.set_account_code(suwappu, account.info.code_hash.0);
                }
            }
        }

        for (slot, value) in &account.storage {
            if value.is_changed() {
                bridge.set_storage(
                    suwappu,
                    slot.to_be_bytes::<32>(),
                    value.present_value.to_be_bytes::<32>(),
                );
            }
        }
    }
    Ok(())
}

/// Real EVM executor (monad-revm). Drop-in for [`MockEvm`](crate::vm::MockEvm).
#[derive(Debug, Default, Clone, Copy)]
pub struct RevmExecutor;

impl RevmExecutor {
    /// Execute an EVM-shape value transfer against `state` through real revm.
    ///
    /// # Errors
    ///
    /// [`EvmError::Revert`] if revm rejects the transaction or it
    /// reverts/halts. State is untouched on error.
    pub fn execute(self, state: &mut State, tx: EvmTx) -> Result<(), EvmError> {
        self.run(
            state,
            RevmAddress::from(tx.from.0),
            TxKind::Call(RevmAddress::from(tx.to.0)),
            tx.value,
            Bytes::new(),
            TRANSFER_GAS_LIMIT,
        )
    }

    /// Execute a contract call: `caller` invokes `target` with `calldata`
    /// and `value`, running the target's bytecode on real revm.
    ///
    /// # Errors
    ///
    /// [`EvmError::Revert`] if revm rejects the transaction or it
    /// reverts/halts. State is untouched on error.
    pub fn execute_call(
        self,
        state: &mut State,
        caller: Address,
        target: Address,
        value: u128,
        calldata: Vec<u8>,
    ) -> Result<(), EvmError> {
        self.run(
            state,
            RevmAddress::from(caller.0),
            TxKind::Call(RevmAddress::from(target.0)),
            value,
            Bytes::from(calldata),
            CALL_GAS_LIMIT,
        )
    }

    /// Shared path: build the Monad EVM over a read-only view, transact, and
    /// (on success) write the diff back. The immutable borrow ends before
    /// the mutable write-back.
    #[allow(clippy::unused_self)] // carried for symmetry with the public methods
    fn run(
        self,
        state: &mut State,
        caller: RevmAddress,
        kind: TxKind,
        value: u128,
        data: Bytes,
        gas_limit: u64,
    ) -> Result<(), EvmError> {
        let sender_nonce = state.slot_of(&Address(caller.into_array())).nonce().value;

        let (result, diff) = {
            let db = SuwappuStateDb { state: &*state };
            let mut evm = monad_context_with_db(WrapDatabaseRef(db)).build_monad();
            let txenv = TxEnv::builder()
                .caller(caller)
                .kind(kind)
                .value(U256::from(value))
                .data(data)
                .gas_limit(gas_limit)
                .gas_price(0)
                .nonce(sender_nonce)
                .build_fill();
            let out = evm
                .transact(txenv)
                .map_err(|_| EvmError::Revert(RejectReason::InsufficientBalance))?;
            (out.result, out.state)
        };

        if !result.is_success() {
            return Err(EvmError::Revert(RejectReason::InsufficientBalance));
        }

        write_back(state, diff)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use suwappudb_state::{BridgeToken, StateChange};

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

        assert_eq!(state.balance_of(&alice), Balance(70));
        assert_eq!(state.balance_of(&bob), Balance(30));
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
        assert_eq!(state.slot_of(&alice).nonce().value, 0);
    }

    #[test]
    fn real_evm_contract_call_writes_storage() {
        use revm::primitives::keccak256;

        let caller = addr(1);
        let contract = addr(2);
        let mut state = seeded(caller, 1_000_000);

        // Runtime bytecode: `storage[0] = calldata[0:32]`
        //   PUSH1 0x00 ; CALLDATALOAD ; PUSH1 0x00 ; SSTORE ; STOP
        let code = vec![0x60, 0x00, 0x35, 0x60, 0x00, 0x55, 0x00];
        let code_hash = keccak256(&code);
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash: code_hash.0,
                code: code.clone(),
            },
        );
        state.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash: code_hash.0,
            },
        );

        // Call the contract with a 32-byte argument (value = 42).
        let mut calldata = [0u8; 32];
        calldata[31] = 42;
        RevmExecutor
            .execute_call(&mut state, caller, contract, 0, calldata.to_vec())
            .unwrap();

        // Real revm ran the bytecode and the SSTORE persisted via write-back.
        assert_eq!(state.storage_at(&contract, &[0u8; 32]), calldata);
        // Caller's nonce advanced.
        assert_eq!(state.slot_of(&caller).nonce().value, 1);
    }

    /// A transfer whose recipient would exceed `u128::MAX` reverts (it must
    /// NOT saturate the recipient to `u128::MAX`, which would let the sender
    /// be debited without the recipient being fully credited). State is
    /// untouched on the revert.
    #[test]
    fn real_evm_balance_overflow_reverts_untouched() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 10);
        // Seed bob just under the u128 ceiling so the incoming transfer
        // would push his post-state balance above u128::MAX.
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: bob,
                to: Balance(u128::MAX - 5),
            },
        );

        let err = RevmExecutor
            .execute(
                &mut state,
                EvmTx {
                    from: alice,
                    to: bob,
                    value: 10,
                },
            )
            .unwrap_err();

        assert_eq!(err, EvmError::Revert(RejectReason::BalanceOverflow));
        // Atomic revert: neither balance moved and the nonce did not advance.
        assert_eq!(state.balance_of(&alice), Balance(10));
        assert_eq!(state.balance_of(&bob), Balance(u128::MAX - 5));
        assert_eq!(state.slot_of(&alice).nonce().value, 0);
    }

    /// A successful real-EVM transfer is supply-neutral: the total across
    /// the involved accounts is identical before and after (no mint/burn).
    #[test]
    fn real_evm_transfer_conserves_supply() {
        let alice = addr(1);
        let bob = addr(2);
        let mut state = seeded(alice, 100);

        let before = state.balance_of(&alice).0 + state.balance_of(&bob).0;
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
        let after = state.balance_of(&alice).0 + state.balance_of(&bob).0;

        assert_eq!(before, 100);
        assert_eq!(after, before, "real EVM transfer must not mint or burn");
    }
}
