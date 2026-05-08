//! **S4 EXIT GATE** — block-level CE-MVCC OCC equivalence under load.
//!
//! For any block (ordered Vec<Intent>) applied to a seeded state,
//! parallel block execution via [`BlockExecutor`] produces the same
//! final state as sequential one-by-one [`Bridge::submit`] over the
//! same intents. The dual-projection invariant (S2/S3) holds at every
//! commit point.
//!
//! Default: 256 cases, runs in seconds.
//!
//! Exit-gate run (release):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --release --test block_executor \
//!       parallel_equals_sequential
//! ```

use gsxdb_bridge::{BlockExecutor, Bridge, Intent, TxOutcome};
use gsxdb_state::{
    Address, Balance, BridgeToken, EvmProjector, EvmView, MoveProjector, MoveView, State,
    StateChange,
};
use proptest::prelude::*;

const ADDR_SPACE: u8 = 8;
const SEED_BALANCE: u128 = 1_000_000;

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

fn intent() -> impl Strategy<Value = Intent> {
    (small_address(), small_address(), 0u128..(SEED_BALANCE * 4))
        .prop_map(|(from, to, amount)| Intent::Transfer { from, to, amount })
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

fn run_sequentially(block: &[Intent]) -> State {
    let mut state = seeded_state();
    for intent in block {
        let mut bridge = Bridge::new(&mut state);
        let _ = bridge.submit(intent.clone());
    }
    state
}

fn run_in_parallel(block: &[Intent]) -> (State, Vec<TxOutcome>) {
    let mut state = seeded_state();
    let report = BlockExecutor.execute(&mut state, block);
    (state, report.outcomes)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S4 EXIT GATE.** Parallel block execution and sequential one-by-one
    /// `Bridge::submit` produce the same final state for any block.
    ///
    /// This is the load-bearing claim of CE-MVCC OCC: that we get
    /// parallelism without changing semantics. Both paths must agree on
    /// every address in the seeded space.
    #[test]
    fn parallel_equals_sequential(
        block in prop::collection::vec(intent(), 0..16),
    ) {
        let s_seq = run_sequentially(&block);
        let (s_par, _) = run_in_parallel(&block);

        for n in 0..ADDR_SPACE {
            let a = Address([n; 20]);
            prop_assert_eq!(s_par.balance_of(&a), s_seq.balance_of(&a));
        }
    }

    /// Dual-projection invariant survives parallel execution. EVM and Move
    /// projections agree on every address after any block.
    #[test]
    fn dual_projection_holds_after_block(
        block in prop::collection::vec(intent(), 0..16),
    ) {
        let (state, _) = run_in_parallel(&block);
        assert_dual_projection(&state);
    }

    /// Sum of all balances is conserved (or strictly decreases — but never
    /// increases — under reject-only blocks). With our seeded state and
    /// transfer-only intents, total supply is exactly preserved.
    ///
    /// This is a structural sanity check: a bug that double-credits or
    /// drops balances during the OCC retry loop would surface as a sum
    /// mismatch.
    #[test]
    fn total_supply_preserved(
        block in prop::collection::vec(intent(), 0..16),
    ) {
        let (state, _) = run_in_parallel(&block);
        let total: u128 = (0..ADDR_SPACE)
            .map(|n| state.balance_of(&Address([n; 20])).0)
            .sum();
        let expected = u128::from(ADDR_SPACE) * SEED_BALANCE;
        prop_assert_eq!(total, expected);
    }

    /// Idempotence under empty blocks: applying an empty block leaves
    /// state untouched no matter how the OCC machinery is exercised.
    #[test]
    fn empty_block_is_identity(seed_amount in 0u128..1_000_000) {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        for n in 0..ADDR_SPACE {
            state.apply(&token, &StateChange::SetBalance {
                addr: Address([n; 20]),
                to: Balance(seed_amount),
            });
        }
        let report = BlockExecutor.execute(&mut state, &[]);
        prop_assert!(report.outcomes.is_empty());
        prop_assert_eq!(report.iterations, 0);
        for n in 0..ADDR_SPACE {
            prop_assert_eq!(state.balance_of(&Address([n; 20])).0, seed_amount);
        }
    }
}
