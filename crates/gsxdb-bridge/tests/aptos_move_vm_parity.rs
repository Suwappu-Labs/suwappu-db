//! **S9.6 EXIT GATE** — dual-projection invariant under the *real*
//! Aptos Move VM.
//!
//! S3's `interleaved_evm_move_preserves_invariant` proptest shows that
//! mixed EVM + Move-shape transactions preserve `EVM balanceOf ==
//! Move Coin.value` under [`MockEvm`] / [`MockMove`] mock executors.
//! S9.6 strengthens that gate: the same invariant must hold when the
//! Move arm runs through `AptosMoveExecutor` against the canonical
//! `0x1::coin::transfer` Move bytecode (S9.5g) via
//! [`BundleExecutor::execute_with_move_runtime`].
//!
//! Exit-gate run (release):
//! ```text
//!   PROPTEST_CASES=10000 cargo test --release \
//!       --features production-move-executor \
//!       --test aptos_move_vm_parity \
//!       interleaved_evm_aptos_move_preserves_invariant
//! ```
//!
//! Test only compiles + runs under `--features production-move-executor`.
//! Default builds compile this file as an empty module so `cargo test`
//! continues to work without the (heavy) Aptos VM dep.

#![cfg(feature = "production-move-executor")]

use gsxdb_bridge::bundle::{Bundle, BundleExecutor, BundleOutcome, BundleStep};
use gsxdb_state::{
    canonical_coin_bytecode, canonical_coin_module_id, AccountNonce, Address, Balance,
    BridgeToken, CompiledModule, EvmProjector, EvmTx, EvmView, Identifier, InMemoryModuleStore,
    ModuleStore as _, MoveAddress, MoveBalanceView, MoveCoinValue, MoveCall, MoveProjector,
    MoveView, State, StateChange,
};
use proptest::prelude::*;

/// Address space restricted to encourage overlap. Same shape as the
/// S3 cross_vm_parity proptest.
const ADDR_SPACE: u8 = 8;
const SEED_BALANCE: u128 = 1_000_000;

/// Build the 32-byte MoveAddress whose canonical EVM projection (last
/// 20 bytes) equals `Address([byte; 20])`. The bridge applies Move
/// `ResourceWrite`s to the substrate via `Address(bytes[12..32])`.
fn move_addr_for(byte: u8) -> MoveAddress {
    let mut bytes = [0u8; 32];
    for b in &mut bytes[12..32] {
        *b = byte;
    }
    MoveAddress(bytes)
}

/// Snapshot `MoveBalanceView` taken from `&State` once, then detached
/// so the bundle executor can take `&mut State` while we hold the
/// view. Mirrors the S9.4 in-crate test fixture.
#[derive(Debug)]
struct SnapshotView {
    balances: std::collections::HashMap<MoveAddress, (u128, u64)>,
}

impl SnapshotView {
    fn from_state(state: &State) -> Self {
        let mut balances = std::collections::HashMap::new();
        for n in 0..ADDR_SPACE {
            let ma = move_addr_for(n);
            let evm = Address([n; 20]);
            balances.insert(ma, (state.balance_of(&evm).0, 0));
        }
        Self { balances }
    }
}

impl MoveBalanceView for SnapshotView {
    fn coin_value(&self, addr: &MoveAddress) -> MoveCoinValue {
        MoveCoinValue::from_u128(self.balances.get(addr).map(|(v, _)| *v).unwrap_or(0))
    }
    fn nonce(&self, addr: &MoveAddress) -> AccountNonce {
        AccountNonce::new(self.balances.get(addr).map(|(_, n)| *n).unwrap_or(0))
    }
}

#[derive(Debug, Clone, Copy)]
enum AptosTx {
    Evm(EvmTx),
    /// `0x1::coin::transfer(from, to, amount)` via AptosMoveExecutor.
    /// Amount is `u64` to match the Move signature.
    Move {
        from: u8,
        to: u8,
        amount: u64,
    },
}

fn small_address() -> impl Strategy<Value = u8> {
    0u8..ADDR_SPACE
}

fn evm_tx() -> impl Strategy<Value = AptosTx> {
    (small_address(), small_address(), any::<u64>()).prop_map(|(from, to, value)| {
        AptosTx::Evm(EvmTx {
            from: Address([from; 20]),
            to: Address([to; 20]),
            value: u128::from(value),
        })
    })
}

fn move_tx() -> impl Strategy<Value = AptosTx> {
    (small_address(), small_address(), any::<u64>())
        .prop_map(|(from, to, amount)| AptosTx::Move { from, to, amount })
}

fn mixed_tx() -> impl Strategy<Value = AptosTx> {
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

/// Build the move call for a transfer. Arguments are BCS-encoded:
/// - from: 32-byte address
/// - to: 32-byte address
/// - amount: u64 LE (8 bytes)
fn transfer_call(from: u8, to: u8, amount: u64) -> MoveCall {
    let from_addr = move_addr_for(from);
    let to_addr = move_addr_for(to);
    MoveCall {
        caller: from_addr,
        module: canonical_coin_module_id(),
        function: Identifier::new("transfer").unwrap(),
        type_arguments: Vec::new(),
        arguments: vec![
            from_addr.0.to_vec(),
            to_addr.0.to_vec(),
            amount.to_le_bytes().to_vec(),
        ],
    }
}

fn execute(
    state: &mut State,
    modules: &mut InMemoryModuleStore,
    tx: AptosTx,
) -> BundleOutcome {
    let bundle = match tx {
        AptosTx::Evm(evm) => Bundle::single(BundleStep::Evm(evm)),
        AptosTx::Move { from, to, amount } => {
            Bundle::single(BundleStep::MoveCall(transfer_call(from, to, amount)))
        }
    };
    let executor = gsxdb_state::AptosMoveExecutor;
    let view = SnapshotView::from_state(state);
    let result =
        BundleExecutor.execute_with_move_runtime(state, &bundle, &executor, modules, &view);
    result.outcome
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

fn pre_deployed_modules() -> InMemoryModuleStore {
    let mut modules = InMemoryModuleStore::new();
    modules
        .put(
            canonical_coin_module_id(),
            CompiledModule {
                bytes: canonical_coin_bytecode().to_vec(),
            },
        )
        .expect("canonical coin deploys cleanly");
    modules
}

proptest! {
    #![proptest_config(ProptestConfig {
        // Modest default; the exit-gate run uses PROPTEST_CASES=10000.
        // Real Move VM is slower than mocks (deserialize + verify +
        // load + interpret per call), but still well under a minute
        // for 1k cases.
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S9.6 EXIT GATE** — interleaved EVM + Aptos-Move transaction
    /// sequences preserve the dual-projection invariant at every step.
    ///
    /// Move arm executes the canonical `0x1::coin::transfer` bytecode
    /// through AptosMoveExecutor → MoveVM::execute_loaded_function and
    /// applies the resulting `ResourceWrite`s back to substrate via
    /// the bridge token.
    #[test]
    fn interleaved_evm_aptos_move_preserves_invariant(
        ops in prop::collection::vec(mixed_tx(), 0..16),
    ) {
        let mut state = seeded_state();
        let mut modules = pre_deployed_modules();
        assert_dual_projection(&state);

        for op in ops {
            let _ = execute(&mut state, &mut modules, op);
            assert_dual_projection(&state);
        }
    }

    /// Move-only sequences preserve the invariant. Symmetric to the
    /// S3 `move_only_preserves_invariant` but driving the real
    /// Aptos VM.
    #[test]
    fn aptos_move_only_preserves_invariant(
        ops in prop::collection::vec(move_tx(), 0..16),
    ) {
        let mut state = seeded_state();
        let mut modules = pre_deployed_modules();
        for op in ops {
            let _ = execute(&mut state, &mut modules, op);
            assert_dual_projection(&state);
        }
    }
}

/// Sanity smoke: a single successful transfer via AptosMoveExecutor
/// debits the source and credits the destination by the same amount.
/// Belongs alongside the proptest because it's the minimum positive
/// case that proves the resource-write extraction path lands writes
/// at the right address.
#[test]
fn aptos_move_transfer_debits_and_credits_real_addresses() {
    let mut state = seeded_state();
    let mut modules = pre_deployed_modules();
    let pre_from = state.balance_of(&Address([0; 20])).0;
    let pre_to = state.balance_of(&Address([1; 20])).0;

    let outcome = execute(
        &mut state,
        &mut modules,
        AptosTx::Move {
            from: 0,
            to: 1,
            amount: 250,
        },
    );

    match outcome {
        BundleOutcome::Committed => {}
        BundleOutcome::Reverted { failed_step } => panic!(
            "expected commit; reverted at step {failed_step}. \
             AptosMoveExecutor or BalanceViewResolver path may be broken."
        ),
    }

    let post_from = state.balance_of(&Address([0; 20])).0;
    let post_to = state.balance_of(&Address([1; 20])).0;
    assert_eq!(pre_from - post_from, 250, "from debit mismatch");
    assert_eq!(post_to - pre_to, 250, "to credit mismatch");
    assert_dual_projection(&state);
}
