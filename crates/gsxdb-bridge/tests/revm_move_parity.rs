//! **EVM EXIT GATE** — dual-projection invariant under the *real* revm.
//!
//! S3's `cross_vm_parity` shows mixed EVM + Move transfers preserve
//! `EVM balanceOf == Move Coin.value` under the *mock* executors. This
//! strengthens it: the EVM arm runs through the real [`RevmExecutor`]
//! (gsx-revm / monad-revm), so the invariant must hold when actual revm
//! execution — gas, nonce advance, the MonadNine handler — writes balances
//! back to the canonical `BalanceSlot`.
//!
//! The Move arm stays `MockMove`; pairing real revm against the real Aptos
//! Move VM needs both `production-evm-executor` and `production-move-executor`
//! and is a follow-on. The point here is that swapping the *EVM* executor
//! from mock to real does not perturb the projection the Move side reads.
//!
//! Exit-gate run (release):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --release \
//!       --features production-evm-executor \
//!       --test revm_move_parity interleaved_revm_move_preserves_invariant
//! ```
//!
//! Compiles only under `--features production-evm-executor` (pulls revm).

#![cfg(feature = "production-evm-executor")]

use gsxdb_bridge::vm::RevmExecutor;
use gsxdb_bridge::MockMove;
use gsxdb_state::{
    Address, Balance, BridgeToken, EvmProjector, EvmTx, EvmView, MoveProjector, MoveTx, MoveView,
    State, StateChange,
};
use proptest::prelude::*;
use std::collections::HashMap;

/// Restrict the address space so transactions overlap and touch the same
/// accounts often, instead of every tx hitting a fresh (trivially-equal)
/// address.
const ADDR_SPACE: u8 = 8;

/// Seed balance per account. Far above the per-tx value bound so most
/// transfers execute rather than reverting on insufficient funds.
const SEED_BALANCE: u128 = 1_000_000_000;

/// Per-tx value bound, well under [`SEED_BALANCE`].
const MAX_VALUE: u128 = 2_000_000;

#[derive(Debug, Clone, Copy)]
enum Tx {
    Evm(EvmTx),
    Move(MoveTx),
}

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

fn evm_tx() -> impl Strategy<Value = Tx> {
    (small_address(), small_address(), 0u128..MAX_VALUE).prop_map(|(from, to, value)| {
        // The `nonce` field is overwritten in the test body to track
        // per-sender envelope nonces — real revm validates `tx.nonce ==
        // account.nonce` and would reject every tx after the first
        // from any given sender otherwise. The strategy only supplies
        // a placeholder.
        Tx::Evm(EvmTx {
            from,
            to,
            value,
            nonce: 0,
        })
    })
}

fn move_tx() -> impl Strategy<Value = Tx> {
    (small_address(), small_address(), 0u128..MAX_VALUE).prop_map(|(signer, recipient, amount)| {
        Tx::Move(MoveTx {
            signer,
            recipient,
            amount,
        })
    })
}

fn mixed_tx() -> impl Strategy<Value = Tx> {
    prop_oneof![evm_tx(), move_tx()]
}

fn seeded_state() -> State {
    let mut state = State::default();
    let token = BridgeToken::__for_bridge_only();
    for n in 0..ADDR_SPACE {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([n; 20]),
                to: Balance(SEED_BALANCE),
            },
        );
    }
    state
}

/// Dispatch a single tx, threading the per-sender envelope nonce for
/// real-revm EVM steps. Move steps are untouched (Move has its own
/// sequence-number semantics on the Move-projection side).
///
/// Real revm rejects any EVM tx whose envelope nonce is not equal to
/// the sender's current state nonce. The proptest can't tell the
/// strategy what nonce to use (the nonce depends on prior executions
/// against this same state), so we maintain a tracker here. After a
/// successful EVM tx, the sender's tracker bumps by 1; the underlying
/// state nonce also advances and the next iteration will use the
/// matching envelope nonce.
fn execute(state: &mut State, tx: Tx, evm_nonces: &mut HashMap<Address, u64>) {
    match tx {
        // Errors (insufficient balance, invalid nonce, etc.) revert with
        // no state change — ignore them, exactly as the mock-based gate
        // does. The dual-projection invariant must still hold on the
        // rejected-state path.
        Tx::Evm(mut t) => {
            t.nonce = *evm_nonces.entry(t.from).or_insert(0);
            if RevmExecutor.execute(state, t).is_ok() {
                *evm_nonces.get_mut(&t.from).expect("inserted above") += 1;
            }
        }
        Tx::Move(t) => {
            let _ = MockMove.execute(state, t);
        }
    }
}

fn assert_dual_projection(state: &State) {
    for n in 0..ADDR_SPACE {
        let addr = Address([n; 20]);
        let evm = EvmView.balance_of(state, &addr).to_u128();
        let mv = MoveView.coin_value(state, &addr).to_u128();
        assert_eq!(
            evm, mv,
            "dual-projection disagreement at addr={n}: evm={evm}, move={mv}"
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Real revm builds an EVM per tx, so this is heavier than the
        // mock gate; keep the default modest. The exit-gate run uses
        // PROPTEST_CASES=10000.
        cases: 64,
        .. ProptestConfig::default()
    })]

    /// **EVM EXIT GATE** — interleaved real-revm + Move transfers preserve
    /// the dual-projection invariant at every step.
    #[test]
    fn interleaved_revm_move_preserves_invariant(
        ops in prop::collection::vec(mixed_tx(), 0..24),
    ) {
        let mut state = seeded_state();
        let mut evm_nonces: HashMap<Address, u64> = HashMap::new();
        assert_dual_projection(&state);
        for op in ops {
            execute(&mut state, op, &mut evm_nonces);
            assert_dual_projection(&state);
        }
    }

    /// Real-revm-only sequences preserve the invariant — the Move view is
    /// read after every revm tx, so a divergence in how real execution
    /// writes the canonical balance would surface here.
    #[test]
    fn revm_only_preserves_invariant(
        ops in prop::collection::vec(evm_tx(), 0..24),
    ) {
        let mut state = seeded_state();
        let mut evm_nonces: HashMap<Address, u64> = HashMap::new();
        for op in ops {
            execute(&mut state, op, &mut evm_nonces);
            assert_dual_projection(&state);
        }
    }
}
