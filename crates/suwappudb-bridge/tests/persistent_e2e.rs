//! End-to-end integration: redb-backed `State` driven through the `Bridge`.
//!
//! This is the test that proves the polish slice's value: the full
//! Lane → Bridge → State → `BalanceStore` → redb stack works through one
//! type-safe API path, with no lane code ever touching `suwappudb-state`
//! directly.

use suwappudb_bridge::{Bridge, Intent};
use suwappudb_state::{Address, Balance, BridgeToken, RedbBalanceStore, State, StateChange};
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
    assert_eq!(err, suwappudb_bridge::RejectReason::InsufficientBalance);
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
