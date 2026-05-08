//! **S3 EXIT GATE** — cross-VM dual-projection invariant under load.
//!
//! For any sequence of mixed EVM-shape and Move-shape transactions
//! applied to a single canonical state, the EVM projection and the Move
//! projection of every touched address agree at every step. This is the
//! operational form of Proposition 1 — that
//! `EVM balanceOf == Move Coin.value` always.
//!
//! Default: 256 cases, runs in seconds.
//!
//! Exit-gate run (release):
//!   PROPTEST_CASES=10000 cargo test --release --test cross_vm_parity \
//!       interleaved_evm_move_preserves_invariant
//!
//! All sub-properties below assert the invariant. They differ only in
//! the workload distribution, to make sure no single transaction
//! flavour pathologically dominates the search.
//!
//! Real revm / Move-VM integration is deferred per IQ-2; this test runs
//! against the mock executors. The encoding paths and projection paths
//! are real and end-to-end through the BridgeToken capability gate.

use gsxdb_bridge::{MockEvm, MockMove};
use gsxdb_state::{
    Address, Balance, BridgeToken, EvmProjector, EvmTx, EvmView, MoveProjector, MoveTx, MoveView,
    State, StateChange,
};
use proptest::prelude::*;

/// Restrict the address space so transactions actually overlap and
/// touch the same accounts often. Without this, every random tx hits a
/// fresh address and the invariant is trivially preserved.
const ADDR_SPACE: u8 = 8;

#[derive(Debug, Clone, Copy)]
enum Tx {
    Evm(EvmTx),
    Move(MoveTx),
}

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

fn evm_tx() -> impl Strategy<Value = Tx> {
    (small_address(), small_address(), any::<u128>())
        .prop_map(|(from, to, value)| Tx::Evm(EvmTx { from, to, value }))
}

fn move_tx() -> impl Strategy<Value = Tx> {
    (small_address(), small_address(), any::<u128>()).prop_map(|(signer, recipient, amount)| {
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
    // Seed every account in the address space with a moderate balance
    // so transactions have something to spend. Without seed funds the
    // search wanders past every error path and exercises nothing.
    let mut state = State::default();
    let token = BridgeToken::__for_bridge_only();
    for n in 0..ADDR_SPACE {
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: Address([n; 20]),
                to: Balance(1_000_000_000),
            },
        );
    }
    state
}

fn execute(state: &mut State, tx: Tx) {
    match tx {
        Tx::Evm(t) => {
            let _ = MockEvm.execute(state, t);
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
        // Modest default; the exit-gate run uses PROPTEST_CASES=10000 in
        // release. Mock executors are pure CPU so 10k still runs fast.
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S3 EXIT GATE** — interleaved EVM + Move transaction sequences
    /// preserve the dual-projection invariant at every step.
    #[test]
    fn interleaved_evm_move_preserves_invariant(
        ops in prop::collection::vec(mixed_tx(), 0..32),
    ) {
        let mut state = seeded_state();
        assert_dual_projection(&state);

        for op in ops {
            execute(&mut state, op);
            assert_dual_projection(&state);
        }
    }

    /// EVM-only sequences preserve the invariant. The MoveView is read
    /// after every EVM tx; if the projection paths diverge under
    /// EVM-shape input only, this catches it.
    #[test]
    fn evm_only_preserves_invariant(
        ops in prop::collection::vec(evm_tx(), 0..32),
    ) {
        let mut state = seeded_state();
        for op in ops {
            execute(&mut state, op);
            assert_dual_projection(&state);
        }
    }

    /// Move-only sequences preserve the invariant. Symmetric to the
    /// above; catches divergence under Move-shape input only.
    #[test]
    fn move_only_preserves_invariant(
        ops in prop::collection::vec(move_tx(), 0..32),
    ) {
        let mut state = seeded_state();
        for op in ops {
            execute(&mut state, op);
            assert_dual_projection(&state);
        }
    }

    /// Same logical operation in both VM shapes produces identical
    /// state. Proves the encoding paths converge before any executor
    /// runs — catches a bug where (say) EvmTx::to_canonical and
    /// MoveTx::to_canonical disagree on field ordering.
    #[test]
    fn evm_and_move_canonical_equivalents_match(
        from in small_address(),
        to in small_address(),
        amount in any::<u128>(),
    ) {
        let mut s_evm = seeded_state();
        let mut s_move = seeded_state();

        let _ = MockEvm.execute(&mut s_evm, EvmTx { from, to, value: amount });
        let _ = MockMove.execute(&mut s_move, MoveTx { signer: from, recipient: to, amount });

        for n in 0..ADDR_SPACE {
            let addr = Address([n; 20]);
            prop_assert_eq!(
                s_evm.balance_of(&addr),
                s_move.balance_of(&addr)
            );
        }
    }
}
