//! End-to-end integration: redb-backed `State` driven through the `Bridge`.
//!
//! This is the test that proves the polish slice's value: the full
//! Lane → Bridge → State → `BalanceStore` → redb stack works through one
//! type-safe API path, with no lane code ever touching `gsxdb-state`
//! directly.

use gsxdb_bridge::{Bridge, Intent};
use gsxdb_state::{Address, Balance, BridgeToken, RedbBalanceStore, State, StateChange};
use tempfile::TempDir;

fn fresh_redb_state() -> (State, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let store = RedbBalanceStore::open(dir.path().join("state.redb")).expect("open redb");
    (State::with_store(Box::new(store)), dir)
}

fn seed(state: &mut State, addr: Address, amount: u128) {
    let token = BridgeToken::__for_bridge_only();
    state.apply(
        &token,
        &StateChange::SetBalance {
            addr,
            to: Balance(amount),
        },
    );
}

#[test]
fn transfer_through_bridge_persists_to_redb() {
    let (mut state, _dir) = fresh_redb_state();
    let alice = Address([1; 20]);
    let bob = Address([2; 20]);

    seed(&mut state, alice, 100);

    let mut bridge = Bridge::new(&mut state);
    bridge
        .submit(Intent::Transfer {
            from: alice,
            to: bob,
            amount: 30,
        })
        .expect("transfer");

    assert_eq!(bridge.balance_of(&alice), Balance(70));
    assert_eq!(bridge.balance_of(&bob), Balance(30));
}

#[test]
fn rejected_transfer_leaves_redb_unchanged() {
    let (mut state, _dir) = fresh_redb_state();
    let alice = Address([1; 20]);
    let bob = Address([2; 20]);

    seed(&mut state, alice, 5);

    let mut bridge = Bridge::new(&mut state);
    let err = bridge
        .submit(Intent::Transfer {
            from: alice,
            to: bob,
            amount: 30,
        })
        .unwrap_err();

    // Source unchanged, destination still zero.
    assert_eq!(bridge.balance_of(&alice), Balance(5));
    assert_eq!(bridge.balance_of(&bob), Balance(0));
    assert_eq!(err, gsxdb_bridge::RejectReason::InsufficientBalance);
}

#[test]
fn state_persists_across_reopen() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.redb");
    let alice = Address([1; 20]);
    let bob = Address([2; 20]);

    {
        let store = RedbBalanceStore::open(&path).unwrap();
        let mut state = State::with_store(Box::new(store));
        seed(&mut state, alice, 1000);

        let mut bridge = Bridge::new(&mut state);
        bridge
            .submit(Intent::Transfer {
                from: alice,
                to: bob,
                amount: 250,
            })
            .unwrap();
    }

    let store = RedbBalanceStore::open(&path).unwrap();
    let state = State::with_store(Box::new(store));
    assert_eq!(state.balance_of(&alice), Balance(750));
    assert_eq!(state.balance_of(&bob), Balance(250));
}

#[test]
fn evm_contract_state_persists_across_reopen() {
    // Contract code, storage, and the account-code pointer must survive a
    // redb reopen alongside balances — otherwise a restarted node loses
    // contract state and its combined state_root silently diverges.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.redb");
    let contract = Address([9; 20]);
    let code_hash = [0xC0u8; 32];
    let code = vec![0x60u8, 0x00, 0x55];
    let slot = [1u8; 32];
    let value = [0xABu8; 32];

    let root_before = {
        let store = RedbBalanceStore::open(&path).unwrap();
        let mut state = State::with_store(Box::new(store));
        let token = BridgeToken::__for_bridge_only();
        seed(&mut state, Address([1; 20]), 1000);
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code: code.clone(),
            },
        );
        state.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash,
            },
        );
        state.apply(
            &token,
            &StateChange::SetStorage {
                addr: contract,
                slot,
                value,
            },
        );
        state.state_root()
    };

    // A fresh State over the same redb file must hydrate the EVM maps.
    let store = RedbBalanceStore::open(&path).unwrap();
    let state = State::with_store(Box::new(store));
    assert_eq!(state.code_by_hash(&code_hash), Some(code.as_slice()));
    assert_eq!(state.account_code_hash(&contract), Some(code_hash));
    assert_eq!(state.storage_at(&contract, &slot), value);
    // The combined root is identical after reopen — contract state is
    // durably bound, not lost like before.
    assert_eq!(state.state_root(), root_before);
}

#[test]
fn zero_storage_write_clears_durable_slot() {
    // A zero value clears the slot durably, matching the in-memory
    // canonicalization, so a reopened node's root agrees.
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("state.redb");
    let contract = Address([9; 20]);
    let slot = [1u8; 32];

    {
        let store = RedbBalanceStore::open(&path).unwrap();
        let mut state = State::with_store(Box::new(store));
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetStorage {
                addr: contract,
                slot,
                value: [7u8; 32],
            },
        );
        state.apply(
            &token,
            &StateChange::SetStorage {
                addr: contract,
                slot,
                value: [0u8; 32],
            },
        );
    }

    let store = RedbBalanceStore::open(&path).unwrap();
    let state = State::with_store(Box::new(store));
    assert_eq!(state.storage_at(&contract, &slot), [0u8; 32]);
    assert!(state.evm_storage_entries().is_empty());
}

#[test]
fn dual_projection_visible_through_state_slot_of() {
    let (mut state, _dir) = fresh_redb_state();
    let alice = Address([1; 20]);

    seed(&mut state, alice, 12345);

    let slot = state.slot_of(&alice);
    assert_eq!(slot.canonical(), 12345);
    assert_eq!(
        slot.evm_balance().to_u128(),
        slot.move_coin_value().to_u128()
    );
}
