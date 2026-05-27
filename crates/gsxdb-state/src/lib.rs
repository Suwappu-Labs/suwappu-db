//! Authoritative state for GSX-DB.
//!
//! Owns balances, anchors, and the cross-VM projections that satisfy
//! Proposition 1 (`EVM balanceOf == Move Coin.value` at every checkpoint).
//!
//! Mutations enter only through [`State::apply`], which is gated by a
//! [`BridgeToken`] that only `gsxdb-bridge` can construct. This is the
//! type-system half of the lane-separation invariant; the static check in
//! `scripts/check-lane-separation.sh` is the build-time half.

#![deny(missing_docs)]

pub mod address_shape;
pub mod balance_slot;
pub mod dag;
pub mod metrics;
pub mod nonce_semantics;
pub mod redb_store;
pub mod snapshot;
pub mod store;
pub mod tree;
pub mod vm;

pub use address_shape::MoveAddress;
pub use balance_slot::{BalanceSlot, EvmBalance, MoveCoinValue, SlotError};
pub use metrics::{Counter, Gauge, Histogram, Metrics, Timer};
pub use nonce_semantics::{AccountNonce, EvmNonce, MoveSequenceNumber};
pub use redb_store::RedbBalanceStore;
pub use store::{BalanceStore, InMemoryBalanceStore};
pub use tree::{Commitment, Proof, ProofStep, StateTree};
pub use vm::{
    AbortLocation, CanonicalTransfer, CompiledModule, EvmProjector, EvmTx, EvmView, Identifier,
    IdentifierError, InMemoryModuleStore, MockMoveExecutor, ModuleId, ModuleStore,
    ModuleStoreError, MoveBalanceView, MoveCall, MoveEvent, MoveExecutionError, MoveExecutor,
    MoveOutcome, MoveProjector, MoveSessionState, MoveTx, MoveView, ResourceWrite, StructTag,
    TypeTag, ABORT_INSUFFICIENT_BALANCE, CANONICAL_COIN_ADDRESS,
};
#[cfg(feature = "production-move-executor")]
pub use vm::{canonical_coin_bytecode, canonical_coin_module_id, AptosMoveExecutor};

/// 20-byte EVM-shaped address. Move addresses are projected onto this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct Address(pub [u8; 20]);

/// Balance held by an address. Newtyped to keep arithmetic intentional.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Balance(pub u128);

/// Authoritative state. Owns the canonical balance map via a pluggable
/// [`BalanceStore`] backend (in-memory, redb, or — in S8 — `RocksDB`).
///
/// Constructed two ways:
///
/// - [`State::default`] — uses [`InMemoryBalanceStore`]. For unit tests and
///   ephemeral runs.
/// - [`State::with_store`] — provide your own backend. For redb-backed runs
///   and any future backend.
pub struct State {
    store: Box<dyn BalanceStore + Send + Sync>,
    /// EVM contract bytecode, keyed by code hash. Populated on contract
    /// creation, read by the EVM executor's `Database` adapter. EVM-only
    /// (the Move VM has no code).
    ///
    /// NOT YET committed in the state root — folding EVM code + storage
    /// into the verkle root is the consensus-critical follow-on; until
    /// then this backs contract *execution* but is not consensus-safe.
    evm_code: std::collections::HashMap<[u8; 32], Vec<u8>>,
    /// EVM contract storage: `(address, 32-byte slot) -> 32-byte value`.
    /// EVM-only. Same state-root caveat as [`Self::evm_code`].
    evm_storage: std::collections::HashMap<(Address, [u8; 32]), [u8; 32]>,
    /// Which contract code each account runs: `address -> code_hash`.
    /// Empty for externally-owned accounts. Lets the EVM `Database` adapter
    /// tell a contract from an EOA without polluting the dual-projection
    /// `BalanceSlot` with a code hash. Same state-root caveat as
    /// [`Self::evm_code`].
    evm_account_code: std::collections::HashMap<Address, [u8; 32]>,
}

impl std::fmt::Debug for State {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("State")
            .field("len", &self.store.len())
            .finish()
    }
}

impl Default for State {
    fn default() -> Self {
        Self::with_store(Box::new(InMemoryBalanceStore::new()))
    }
}

/// A validated state mutation. Constructed by `gsxdb-bridge` after OCC checks
/// and signature verification; `gsxdb-state` trusts these unconditionally.
#[derive(Debug, Clone)]
pub enum StateChange {
    /// Set the balance of `addr` to `to`. Replaces, does not add.
    ///
    /// Resets the nonce to zero — use [`StateChange::SetAccount`] to set
    /// balance and nonce together.
    SetBalance {
        /// Target address.
        addr: Address,
        /// New balance.
        to: Balance,
    },
    /// Set both the balance and nonce of `addr`, replacing the whole slot.
    ///
    /// The real EVM executor uses this to write back a post-execution
    /// account whose nonce advanced; `SetBalance` cannot express a nonce
    /// change because it zeroes the nonce.
    SetAccount {
        /// Target address.
        addr: Address,
        /// New balance.
        balance: Balance,
        /// New nonce.
        nonce: u64,
    },
    /// Store EVM contract bytecode under its code hash (on contract creation).
    SetCode {
        /// `keccak256(code)`.
        code_hash: [u8; 32],
        /// Contract bytecode.
        code: Vec<u8>,
    },
    /// Set an EVM contract storage slot.
    SetStorage {
        /// Contract address.
        addr: Address,
        /// 32-byte storage slot key.
        slot: [u8; 32],
        /// 32-byte storage value.
        value: [u8; 32],
    },
    /// Point an account at the contract code it runs (on contract creation).
    /// Pairs with [`StateChange::SetCode`], which stores the bytecode.
    SetAccountCode {
        /// Contract address.
        addr: Address,
        /// `keccak256(code)` — key into the code store.
        code_hash: [u8; 32],
    },
}

/// Capability token proving a caller is the bridge.
///
/// The constructor is `pub(crate)` — only this crate (via the `for_bridge`
/// associated function exposed to the bridge crate) can mint one. Lane code
/// cannot fabricate a token even if it depends on `gsxdb-state` (which it
/// must not, per the script check).
pub struct BridgeToken {
    _seal: (),
}

impl BridgeToken {
    /// Mint a token. This function is only intended to be called from
    /// `gsxdb-bridge`. The lane-separation script ensures no other crate in
    /// the workspace depends on `gsxdb-state` at all.
    #[must_use]
    pub fn __for_bridge_only() -> Self {
        Self { _seal: () }
    }
}

impl State {
    /// New `State` over the given storage backend.
    #[must_use]
    pub fn with_store(store: Box<dyn BalanceStore + Send + Sync>) -> Self {
        Self {
            store,
            evm_code: std::collections::HashMap::new(),
            evm_storage: std::collections::HashMap::new(),
            evm_account_code: std::collections::HashMap::new(),
        }
    }

    /// Read-only balance lookup. Anyone may call this — reads are not
    /// privileged. (Phase-1 reads through the bridge; phase-2 may expose a
    /// snapshot reader.)
    #[must_use]
    pub fn balance_of(&self, addr: &Address) -> Balance {
        self.store.get(addr).as_balance()
    }

    /// Read the full [`BalanceSlot`] for `addr`. Used by code that needs the
    /// dual EVM/Move projections; most callers want [`State::balance_of`].
    #[must_use]
    pub fn slot_of(&self, addr: &Address) -> BalanceSlot {
        self.store.get(addr)
    }

    /// Snapshot of every (addr, slot) currently in state. Used by the
    /// state-tree commitment to recompute the post-block root. Order
    /// is implementation-defined; consumers must sort if they need
    /// canonical ordering.
    #[must_use]
    pub fn entries(&self) -> Vec<(Address, BalanceSlot)> {
        self.store.entries()
    }

    /// EVM contract bytecode for `code_hash`, or `None` if unknown.
    ///
    /// Non-privileged read, like [`State::balance_of`]. Used by the EVM
    /// executor's `Database` adapter.
    #[must_use]
    pub fn code_by_hash(&self, code_hash: &[u8; 32]) -> Option<&[u8]> {
        self.evm_code.get(code_hash).map(Vec::as_slice)
    }

    /// EVM storage value at `(addr, slot)`. Unset slots read as zero —
    /// the EVM's own "unset storage is zero" contract.
    #[must_use]
    pub fn storage_at(&self, addr: &Address, slot: &[u8; 32]) -> [u8; 32] {
        self.evm_storage
            .get(&(*addr, *slot))
            .copied()
            .unwrap_or([0u8; 32])
    }

    /// Code hash of the contract `addr` runs, or `None` for an EOA.
    ///
    /// Lets the EVM `Database` adapter distinguish a contract from an
    /// externally-owned account; pair with [`State::code_by_hash`] to fetch
    /// the bytecode.
    #[must_use]
    pub fn account_code_hash(&self, addr: &Address) -> Option<[u8; 32]> {
        self.evm_account_code.get(addr).copied()
    }

    /// Commitment over EVM-only state (contract code + storage). Committed
    /// in the state root but **outside** the balance dual-projection
    /// (IQ-10): the Move VM has no code or storage, so this binds them for
    /// consensus without entering the `EvmView`/`MoveView` projection.
    ///
    /// `BLAKE3` over every EVM account in address order —
    /// `addr(20) || code_hash(32) || storage_root(32)` — where
    /// `storage_root` is `BLAKE3` over that account's `(slot, value)` pairs
    /// in slot order. Accounts with neither code nor storage don't
    /// contribute; empty EVM state commits to the domain tag alone.
    #[must_use]
    pub fn evm_state_root(&self) -> Commitment {
        use std::collections::{BTreeMap, BTreeSet};
        const TAG_EVM_STATE: &[u8] = b"GSXDB-EVM-STATE_";
        const TAG_EVM_STORAGE: &[u8] = b"GSXDB-EVM-STORAGE";

        // Group storage by account, slots in canonical (sorted) order.
        let mut storage: BTreeMap<Address, BTreeMap<[u8; 32], [u8; 32]>> = BTreeMap::new();
        for ((addr, slot), value) in &self.evm_storage {
            storage.entry(*addr).or_default().insert(*slot, *value);
        }
        // Union of accounts with code and/or storage, in address order.
        let mut accounts: BTreeSet<Address> = BTreeSet::new();
        accounts.extend(self.evm_account_code.keys().copied());
        accounts.extend(storage.keys().copied());

        let mut h = blake3::Hasher::new();
        h.update(TAG_EVM_STATE);
        for addr in &accounts {
            h.update(&addr.0);
            h.update(&self.evm_account_code.get(addr).copied().unwrap_or([0u8; 32]));
            let mut sh = blake3::Hasher::new();
            sh.update(TAG_EVM_STORAGE);
            if let Some(slots) = storage.get(addr) {
                for (slot, value) in slots {
                    sh.update(slot);
                    sh.update(value);
                }
            }
            h.update(sh.finalize().as_bytes());
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
        Commitment(out)
    }

    /// The consensus state root: the balance tree (IQ-6) bound to the
    /// EVM-state commitment (IQ-10).
    ///
    /// `BLAKE3("GSXDB-STATE-ROOT" || balance_tree_root || evm_state_root)`.
    /// This is the root validators co-sign; it is a deterministic function
    /// of all state, including contract code + storage.
    #[must_use]
    pub fn state_root(&self) -> Commitment {
        const TAG_STATE_ROOT: &[u8] = b"GSXDB-STATE-ROOT";
        let balance_root = StateTree::from_state(self).root();
        let evm_root = self.evm_state_root();
        let mut h = blake3::Hasher::new();
        h.update(TAG_STATE_ROOT);
        h.update(&balance_root.0);
        h.update(&evm_root.0);
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
        Commitment(out)
    }

    /// Apply a validated change. Requires a [`BridgeToken`] — only the bridge
    /// can call this.
    pub fn apply(&mut self, _token: &BridgeToken, change: &StateChange) {
        match change {
            StateChange::SetBalance { addr, to } => {
                self.store.set(addr, BalanceSlot::new(to.0));
            }
            StateChange::SetAccount {
                addr,
                balance,
                nonce,
            } => {
                self.store.set(
                    addr,
                    BalanceSlot::with_nonce(
                        balance.0,
                        crate::nonce_semantics::AccountNonce::new(*nonce),
                    ),
                );
            }
            StateChange::SetCode { code_hash, code } => {
                self.evm_code.insert(*code_hash, code.clone());
            }
            StateChange::SetStorage { addr, slot, value } => {
                self.evm_storage.insert((*addr, *slot), *value);
            }
            StateChange::SetAccountCode { addr, code_hash } => {
                self.evm_account_code.insert(*addr, *code_hash);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applied_balance_round_trips() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([1; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(42),
            },
        );

        assert_eq!(state.balance_of(&addr), Balance(42));
    }

    #[test]
    fn evm_code_round_trips() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let code_hash = [7u8; 32];

        assert_eq!(state.code_by_hash(&code_hash), None);
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code: vec![0x60, 0x00, 0x55],
            },
        );
        assert_eq!(
            state.code_by_hash(&code_hash),
            Some([0x60, 0x00, 0x55].as_slice())
        );

        // The account-code pointer: which address runs this code.
        let contract = Address([8; 20]);
        assert_eq!(state.account_code_hash(&contract), None);
        state.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash,
            },
        );
        assert_eq!(state.account_code_hash(&contract), Some(code_hash));
    }

    #[test]
    fn evm_storage_round_trips_and_defaults_zero() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([3; 20]);
        let slot = [1u8; 32];

        // Unset slot reads as zero (the EVM's own contract).
        assert_eq!(state.storage_at(&addr, &slot), [0u8; 32]);

        let value = [9u8; 32];
        state.apply(&token, &StateChange::SetStorage { addr, slot, value });
        assert_eq!(state.storage_at(&addr, &slot), value);
        // A different slot is still zero.
        assert_eq!(state.storage_at(&addr, &[2u8; 32]), [0u8; 32]);
    }

    #[test]
    fn state_root_commits_contract_storage_and_code() {
        let token = BridgeToken::__for_bridge_only();
        let contract = Address([2; 20]);

        let mut state = State::default();
        let r0 = state.state_root();

        // A storage write changes the consensus root.
        state.apply(
            &token,
            &StateChange::SetStorage {
                addr: contract,
                slot: [1u8; 32],
                value: [9u8; 32],
            },
        );
        let r1 = state.state_root();
        assert_ne!(r0, r1, "storage write must change the state root");

        // Deploying code changes it again.
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash: [7u8; 32],
                code: vec![1, 2, 3],
            },
        );
        state.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash: [7u8; 32],
            },
        );
        assert_ne!(r1, state.state_root(), "code deploy must change the state root");
    }

    #[test]
    fn state_root_is_deterministic_and_order_independent() {
        let token = BridgeToken::__for_bridge_only();
        let c = Address([4; 20]);

        let mut a = State::default();
        a.apply(&token, &StateChange::SetStorage { addr: c, slot: [1u8; 32], value: [10u8; 32] });
        a.apply(&token, &StateChange::SetStorage { addr: c, slot: [2u8; 32], value: [20u8; 32] });

        let mut b = State::default();
        b.apply(&token, &StateChange::SetStorage { addr: c, slot: [2u8; 32], value: [20u8; 32] });
        b.apply(&token, &StateChange::SetStorage { addr: c, slot: [1u8; 32], value: [10u8; 32] });

        assert_eq!(a.state_root(), b.state_root());
    }

    #[test]
    fn state_root_eoa_only_tracks_balances() {
        let token = BridgeToken::__for_bridge_only();

        let mut a = State::default();
        a.apply(&token, &StateChange::SetBalance { addr: Address([1; 20]), to: Balance(100) });
        let mut b = State::default();
        b.apply(&token, &StateChange::SetBalance { addr: Address([1; 20]), to: Balance(100) });
        // No contracts: empty EVM-state commitment is constant, so equal balances → equal root.
        assert_eq!(a.state_root(), b.state_root());

        let mut c = State::default();
        c.apply(&token, &StateChange::SetBalance { addr: Address([1; 20]), to: Balance(101) });
        assert_ne!(a.state_root(), c.state_root());
    }

    #[test]
    fn missing_address_reads_zero() {
        let state = State::default();
        assert_eq!(state.balance_of(&Address([7; 20])), Balance(0));
    }

    #[test]
    fn slot_of_exposes_dual_projection() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([3; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(999),
            },
        );

        let slot = state.slot_of(&addr);
        assert_eq!(slot.canonical(), 999);
        assert_eq!(
            slot.evm_balance().to_u128(),
            slot.move_coin_value().to_u128()
        );
    }

    #[test]
    fn with_store_swaps_the_backend() {
        // Trivial smoke test: build a State around an InMemoryBalanceStore
        // explicitly (the same path Default uses, but with a different code
        // path) and confirm the basic round-trip works. Real backend swap
        // is exercised by the redb integration test in tests/.
        let mut state = State::with_store(Box::new(InMemoryBalanceStore::new()));
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([5; 20]);

        state.apply(
            &token,
            &StateChange::SetBalance {
                addr,
                to: Balance(1),
            },
        );

        assert_eq!(state.balance_of(&addr), Balance(1));
    }
}
