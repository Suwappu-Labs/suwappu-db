//! **S8 EXIT GATE** — recovery via replay matches live execution.
//!
//! For any sequence of blocks executed live, recording each block's
//! state root, then replaying from a fresh state via the `BlockStore`
//! must reach the identical end state. Every address in the seeded
//! space must agree.
//!
//! Default: 256 cases.
//!
//! Exit-gate run:
//! ```text
//!   PROPTEST_CASES=10000 cargo test --test recovery \
//!       recover_matches_live_state
//! ```

use suwappudb_bridge::{
    Block, BlockExecutor, BlockHash, BlockStore, ContractRegistry, InMemoryBlockStore, Intent,
};
use suwappudb_state::{Address, Balance, BridgeToken, State, StateChange};
use proptest::prelude::*;

const ADDR_SPACE: u8 = 8;
const SEED_BALANCE: u128 = 1_000_000;

fn small_address() -> impl Strategy<Value = Address> {
    (0u8..ADDR_SPACE).prop_map(|n| Address([n; 20]))
}

fn intent() -> impl Strategy<Value = Intent> {
    (small_address(), small_address(), 0u128..(SEED_BALANCE * 2))
        .prop_map(|(from, to, amount)| Intent::Transfer { from, to, amount })
}

fn block_strategy() -> impl Strategy<Value = Vec<Intent>> {
    prop::collection::vec(intent(), 0..6)
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

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 256,
        .. ProptestConfig::default()
    })]

    /// **S8 EXIT GATE.** Replay reaches the identical end state as
    /// the live block executor.
    #[test]
    fn recover_matches_live_state(
        blocks in prop::collection::vec(block_strategy(), 0..6),
    ) {
        let mut live = seeded_state();
        let mut store = InMemoryBlockStore::new();
        let mut prev_hash = BlockHash([0; 32]);

        for (height, intents) in blocks.iter().enumerate() {
            let report = BlockExecutor.execute(&mut live, intents);
            let block = Block {
                height: u64::try_from(height).unwrap(),
                parent: prev_hash,
                state_root: report.state_root,
                intents: intents.clone(),
            };
            prev_hash = block.hash();
            store.put(block).unwrap();
        }

        let mut replayed = seeded_state();
        suwappudb_bridge::replay(&store, &mut replayed, &ContractRegistry::new(), 0).unwrap();

        prop_assert_eq!(snapshot(&live), snapshot(&replayed));
    }

    /// Replay is deterministic — same store and start state, same
    /// end state, every time.
    #[test]
    fn replay_is_deterministic(
        blocks in prop::collection::vec(block_strategy(), 0..6),
    ) {
        let mut store = InMemoryBlockStore::new();
        let mut prev_hash = BlockHash([0; 32]);
        let mut live = seeded_state();
        for (height, intents) in blocks.iter().enumerate() {
            let report = BlockExecutor.execute(&mut live, intents);
            let block = Block {
                height: u64::try_from(height).unwrap(),
                parent: prev_hash,
                state_root: report.state_root,
                intents: intents.clone(),
            };
            prev_hash = block.hash();
            store.put(block).unwrap();
        }

        let mut a = seeded_state();
        let mut b = seeded_state();
        suwappudb_bridge::replay(&store, &mut a, &ContractRegistry::new(), 0).unwrap();
        suwappudb_bridge::replay(&store, &mut b, &ContractRegistry::new(), 0).unwrap();
        prop_assert_eq!(snapshot(&a), snapshot(&b));
    }

    /// Tampering with any block's state_root in the store causes
    /// replay to fail with StateRootMismatch.
    #[test]
    fn tampered_state_root_caught(
        blocks in prop::collection::vec(block_strategy(), 1..4),
        tamper_idx in 0usize..4,
    ) {
        let mut live = seeded_state();
        let mut store = InMemoryBlockStore::new();
        let mut prev_hash = BlockHash([0; 32]);
        let mut block_intents = Vec::new();
        for (height, intents) in blocks.iter().enumerate() {
            let report = BlockExecutor.execute(&mut live, intents);
            let block = Block {
                height: u64::try_from(height).unwrap(),
                parent: prev_hash,
                state_root: report.state_root,
                intents: intents.clone(),
            };
            prev_hash = block.hash();
            store.put(block.clone()).unwrap();
            block_intents.push(block);
        }

        // Tamper: rewrite one block's state root via a fresh store.
        let target_idx = tamper_idx % blocks.len();
        let mut tampered_store = InMemoryBlockStore::new();
        let mut new_prev = BlockHash([0; 32]);
        for (i, mut block) in block_intents.iter().cloned().enumerate() {
            block.parent = new_prev;
            if i == target_idx {
                block.state_root = suwappudb_state::Commitment([0xee; 32]);
            }
            new_prev = block.hash();
            tampered_store.put(block).unwrap();
        }

        let mut replayed = seeded_state();
        let result = suwappudb_bridge::replay(
            &tampered_store,
            &mut replayed,
            &ContractRegistry::new(),
            0,
        );
        prop_assert!(result.is_err(), "tampered state_root went undetected");
    }
}
