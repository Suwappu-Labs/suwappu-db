//! **S5 EXIT GATE** — cross-VM bundle atomicity under load.
//!
//! For any random sequence of bundles (each containing 0–8 mixed
//! EVM and Move steps) applied to a seeded state, atomicity holds:
//! a bundle that fails leaves state exactly as if it never executed,
//! a bundle that succeeds is equivalent to applying its steps
//! sequentially. The dual-projection invariant survives across
//! bundles. Total supply is preserved.
//!
//! Default: 256 cases.
//!
//! Exit-gate run (release):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --release --test cross_vm_bundles \
//!       bundle_atomicity
//! ```
//!
//! This proptest exercises the *mock-path* bundle dispatch, which
//! ignores envelope nonces. Under `production-evm-executor` the
//! bundle path dispatches through `RevmExecutor` and would reject
//! every post-first EVM tx from any sender with `InvalidNonce`
//! (degenerate runs that pass atomicity trivially), so the test is
//! compiled out under that feature. The unit tests in
//! `bundle::executor::revm_bundle_tests` (in the gsxdb-bridge lib)
//! cover real-revm bundle dispatch end-to-end, and `revm_move_parity`
//! threads per-sender nonces for the cross-VM real-revm exit gate.

#![cfg(not(feature = "production-evm-executor"))]

use gsxdb_bridge::{Bundle, BundleExecutor, BundleStep, RejectReason, TxOutcome};
use gsxdb_state::{
    Address, Balance, BridgeToken, EvmProjector, EvmTx, EvmView, MoveProjector, MoveTx, MoveView,
    State, StateChange,
};
use proptest::prelude::*;

const ADDR_SPACE: u8 = 8;
const SEED_BALANCE: u128 = 1_000_000;

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

fn evm_step() -> impl Strategy<Value = BundleStep> {
    (small_address(), small_address(), 0u128..(SEED_BALANCE * 4)).prop_map(
        |(from, to, value)| {
            // Without `production-evm-executor`, the bundle path lowers
            // `BundleStep::Evm` to `Intent::Transfer` and the bridge
            // ignores envelope nonce. With the feature on, the bundle
            // dispatches through `RevmExecutor` and real envelope
            // nonces would matter; this proptest runs against the
            // mock-path bundle dispatch by design, so a placeholder
            // nonce is correct here. `revm_move_parity` is the
            // real-revm exit gate.
            BundleStep::Evm(EvmTx {
                from,
                to,
                value,
                nonce: 0,
            })
        },
    )
}

fn move_step() -> impl Strategy<Value = BundleStep> {
    (small_address(), small_address(), 0u128..(SEED_BALANCE * 4)).prop_map(
        |(signer, recipient, amount)| {
            BundleStep::Move(MoveTx {
                signer,
                recipient,
                amount,
            })
        },
    )
}

fn bundle_strategy() -> impl Strategy<Value = Bundle> {
    prop::collection::vec(prop_oneof![evm_step(), move_step()], 0..8)
        .prop_map(|steps| Bundle { steps })
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

fn snapshot(state: &State) -> Vec<u128> {
    (0..ADDR_SPACE)
        .map(|n| state.balance_of(&Address([n; 20])).0)
        .collect()
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
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S5 EXIT GATE.** Bundle atomicity: a bundle that reverts
    /// leaves state exactly as if it never executed.
    ///
    /// Implementation invariant — if any step in a bundle returns
    /// rejected, the post-state must equal the pre-state on every
    /// address.
    #[test]
    fn bundle_atomicity(bundle in bundle_strategy()) {
        let mut state = seeded_state();
        let pre = snapshot(&state);

        let result = BundleExecutor.execute(&mut state, &bundle);
        let post = snapshot(&state);

        if result.is_committed() {
            // Committed = at least one address may have changed.
            // Sub-claim: the change is equivalent to sequential
            // application of the bundle steps. (Tested separately.)
            // Here we just verify no out-of-bundle side effect.
            // (Placeholder: total supply preserved.)
            let pre_sum: u128 = pre.iter().sum();
            let post_sum: u128 = post.iter().sum();
            prop_assert_eq!(pre_sum, post_sum, "supply changed across committed bundle");
        } else {
            // Reverted: state must be exactly pre.
            prop_assert_eq!(pre, post, "reverted bundle leaked state changes");
        }
    }

    /// Bundle equivalence: a successful bundle is equivalent to
    /// sequentially applying each step through the bridge.
    #[test]
    fn bundle_equivalence_to_sequential(bundle in bundle_strategy()) {
        // Pre-flight: skip cases where any step would reject when
        // applied sequentially. Otherwise sequential runs all steps
        // (some succeeding, some failing), but bundle treats any
        // failure as full revert — different terminal state. We
        // restrict to the "all steps individually succeed" subset.
        let mut probe = seeded_state();
        let mut all_succeed = true;
        for step in &bundle.steps {
            let intent = match step {
                BundleStep::Evm(tx) => {
                    let c = tx.to_canonical();
                    gsxdb_bridge::Intent::Transfer { from: c.from, to: c.to, amount: c.amount }
                }
                BundleStep::Move(tx) => {
                    let c = tx.to_canonical();
                    gsxdb_bridge::Intent::Transfer { from: c.from, to: c.to, amount: c.amount }
                }
                // S9.4: MoveCall + DeployModule never appear in the
                // proptest's generated bundles (the strategy emits only
                // Evm/Move transfer steps). Skip them at the type
                // level so the match is exhaustive.
                BundleStep::MoveCall(_) | BundleStep::DeployModule { .. } => continue,
            };
            let mut bridge = gsxdb_bridge::Bridge::new(&mut probe);
            if bridge.submit(intent).is_err() {
                all_succeed = false;
                break;
            }
        }

        if !all_succeed {
            // Non-applicable case; skip.
            return Ok(());
        }

        // Both paths apply the same sequence of successful steps.
        let mut s_bundle = seeded_state();
        let result = BundleExecutor.execute(&mut s_bundle, &bundle);
        prop_assert!(result.is_committed());

        // probe already has the sequential result.
        let bundle_state = snapshot(&s_bundle);
        let seq_state = snapshot(&probe);
        prop_assert_eq!(bundle_state, seq_state);
    }

    /// Dual-projection invariant survives bundle execution. EVM and
    /// Move projections agree on every address after any bundle
    /// (committed or reverted).
    #[test]
    fn dual_projection_holds_across_bundles(bundle in bundle_strategy()) {
        let mut state = seeded_state();
        let _ = BundleExecutor.execute(&mut state, &bundle);
        assert_dual_projection(&state);
    }

    /// Total supply is preserved across any bundle. Catches a
    /// double-credit / drop bug in the rollback path that would
    /// surface as a sum mismatch.
    #[test]
    fn total_supply_preserved_across_bundles(
        bundles in prop::collection::vec(bundle_strategy(), 0..8),
    ) {
        let mut state = seeded_state();
        for bundle in bundles {
            let _ = BundleExecutor.execute(&mut state, &bundle);
        }
        let total: u128 = (0..ADDR_SPACE)
            .map(|n| state.balance_of(&Address([n; 20])).0)
            .sum();
        let expected = u128::from(ADDR_SPACE) * SEED_BALANCE;
        prop_assert_eq!(total, expected);
    }

    /// First-step-fail boundary: if step 0 rejects, the bundle is a
    /// strict no-op. Catches a bug where the snapshot path leaks even
    /// without any speculative writes.
    #[test]
    fn first_step_failure_is_pure_noop(
        invalid_amount in (SEED_BALANCE + 1)..(SEED_BALANCE * 4),
        from in small_address(),
        to in small_address(),
    ) {
        let bundle = Bundle::single(BundleStep::Evm(EvmTx {
            from,
            to,
            value: invalid_amount,
            nonce: 0,
        }));
        let mut state = seeded_state();
        let pre = snapshot(&state);

        let result = BundleExecutor.execute(&mut state, &bundle);
        let post = snapshot(&state);

        prop_assert_eq!(
            result.outcome,
            gsxdb_bridge::BundleOutcome::Reverted { failed_step: 0 }
        );
        prop_assert_eq!(
            result.step_outcomes,
            vec![TxOutcome::Rejected(RejectReason::InsufficientBalance)]
        );
        prop_assert_eq!(pre, post);
    }
}
