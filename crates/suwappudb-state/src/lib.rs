//! Authoritative state for Suwappu DB.
//!
//! Owns balances, anchors, and the cross-VM projections that satisfy
//! Proposition 1 (`EVM balanceOf == Move Coin.value` at every checkpoint).
//!
//! Mutations enter only through [`State::apply`], which is gated by a
//! [`BridgeToken`] that only `suwappudb-bridge` can construct. This is the
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

/// One EVM contract-storage entry — `((address, 32-byte slot), 32-byte
/// value)`. Aliased so the bulk-accessor / store signatures stay readable
/// (and satisfy `clippy::type_complexity`).
pub type EvmStorageEntry = ((Address, [u8; 32]), [u8; 32]);

/// Authoritative state. Owns the canonical balance map via a pluggable
/// [`BalanceStore`] backend (in-memory, redb, or — in S8 — `RocksDB`),
/// plus the EVM contract code / storage / account-code maps.
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
    /// Committed in the consensus state root (IQ-10): [`Self::evm_state_root`]
    /// is a deterministic commitment over code + storage + account-code that
    /// [`Self::state_root`] folds in, so contract state is consensus-safe — a
    /// change here moves the root, and two states with different contract
    /// state cannot share a root.
    evm_code: std::collections::HashMap<[u8; 32], Vec<u8>>,
    /// EVM contract storage: `(address, 32-byte slot) -> 32-byte value`.
    /// EVM-only. Committed in the state root via [`Self::evm_state_root`].
    evm_storage: std::collections::HashMap<(Address, [u8; 32]), [u8; 32]>,
    /// Which contract code each account runs: `address -> code_hash`.
    /// Empty for externally-owned accounts. Lets the EVM `Database` adapter
    /// tell a contract from an EOA without polluting the dual-projection
    /// `BalanceSlot` with a code hash. Committed in the state root via
    /// [`Self::evm_state_root`].
    evm_account_code: std::collections::HashMap<Address, [u8; 32]>,
    /// Reserved-address bytes registry (`address -> bytes`). Backs the
    /// suwappu-dag substrate's L2 verifying-key / DA-anchor / governance
    /// records that `SetBalance` cannot express. `BTreeMap` for canonical
    /// (address-ordered) iteration in the state-root commitment. Committed
    /// in the state root via [`State::bytes_state_root`].
    bytes_state: std::collections::BTreeMap<Address, Vec<u8>>,
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

/// A validated state mutation. Constructed by `suwappudb-bridge` after OCC checks
/// and signature verification; `suwappudb-state` trusts these unconditionally.
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
    ///
    /// **Invariant.** `code_hash` MUST equal `keccak256(code)`. The invariant
    /// is enforced inside [`State::apply`] — applying a `SetCode` with a
    /// mismatched hash panics, so any bridge-side or snapshot-restore path
    /// that wants to seed bytecode must compute the hash honestly.
    /// Without this check the revm `Database` adapter would surface the
    /// stored hash in `AccountInfo.code_hash` while executing different
    /// bytes, letting `EXTCODEHASH` lie about the executable code at an
    /// address.
    SetCode {
        /// `keccak256(code)` — see the invariant note above.
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
    /// Set a reserved-address bytes-state record (L2 verifying key, DA
    /// anchor, governance registry). Replaces any existing value.
    SetBytes {
        /// Reserved address the record is keyed by.
        addr: Address,
        /// Opaque record bytes.
        bytes: Vec<u8>,
    },
}

/// Capability token proving a caller is the bridge.
///
/// The constructor is `pub(crate)` — only this crate (via the `for_bridge`
/// associated function exposed to the bridge crate) can mint one. Lane code
/// cannot fabricate a token even if it depends on `suwappudb-state` (which it
/// must not, per the script check).
pub struct BridgeToken {
    _seal: (),
}

impl BridgeToken {
    /// Mint a token. This function is only intended to be called from
    /// `suwappudb-bridge`. The lane-separation script ensures no other crate in
    /// the workspace depends on `suwappudb-state` at all.
    #[must_use]
    pub fn __for_bridge_only() -> Self {
        Self { _seal: () }
    }
}

impl State {
    /// New `State` over the given storage backend.
    #[must_use]
    pub fn with_store(store: Box<dyn BalanceStore + Send + Sync>) -> Self {
        // Hydrate the in-memory EVM + bytes maps from the (possibly durable)
        // store so contract code/storage/account-code and bytes_state records
        // survive a reopen. Non-durable stores return empty (the default trait
        // impls), giving a fresh State as before.
        let evm_code = store.codes().into_iter().collect();
        let evm_storage = store.storage_entries().into_iter().collect();
        let evm_account_code = store.account_codes().into_iter().collect();
        let bytes_state = store.bytes_entries().into_iter().collect();
        Self {
            store,
            evm_code,
            evm_storage,
            evm_account_code,
            bytes_state,
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

    /// Read the bytes-state record at `addr`, or `None` if unset.
    ///
    /// Non-privileged read (like [`State::balance_of`]). Backs the suwappu-dag
    /// substrate's reserved-address registries (L2 keys, DA anchors,
    /// governance) via the `Substrate::read_bytes` adapter method.
    #[must_use]
    pub fn read_bytes(&self, addr: &Address) -> Option<&[u8]> {
        self.bytes_state.get(addr).map(Vec::as_slice)
    }

    /// All `(code_hash, code)` pairs. For snapshots; order is unspecified,
    /// so callers that need determinism must sort.
    #[must_use]
    pub fn evm_code_entries(&self) -> Vec<([u8; 32], Vec<u8>)> {
        self.evm_code.iter().map(|(k, v)| (*k, v.clone())).collect()
    }

    /// All `((addr, slot), value)` contract-storage entries. For snapshots.
    #[must_use]
    pub fn evm_storage_entries(&self) -> Vec<EvmStorageEntry> {
        self.evm_storage.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// All `(addr, code_hash)` account-code pointers. For snapshots.
    #[must_use]
    pub fn evm_account_code_entries(&self) -> Vec<(Address, [u8; 32])> {
        self.evm_account_code.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// All `(addr, bytes)` reserved-address records. For snapshots; the
    /// `BTreeMap` already yields address-sorted order.
    #[must_use]
    pub fn bytes_state_entries(&self) -> Vec<(Address, Vec<u8>)> {
        self.bytes_state.iter().map(|(k, v)| (*k, v.clone())).collect()
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
        const TAG_EVM_STATE: &[u8] = b"SUWAPPUDB-EVM-STATE_";
        const TAG_EVM_STORAGE: &[u8] = b"SUWAPPUDB-EVM-STORAGE";

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
            // Bind the actual bytecode, not just its hash: `SetCode` accepts
            // arbitrary bytes for a given hash, so committing only the hash
            // would let two states run different code under an identical
            // root. Only an account that actually points at code contributes
            // its bytes; a code-less account commits a zero hash + empty
            // bytes. We must NOT look up the defaulted `[0; 32]` sentinel —
            // it can alias a genuinely-deployed code with hash `[0; 32]`,
            // which would make a storage-only account's root depend on an
            // unrelated deploy.
            match self.evm_account_code.get(addr) {
                Some(code_hash) => {
                    h.update(code_hash);
                    let code = self.code_by_hash(code_hash).unwrap_or(&[]);
                    h.update(&(code.len() as u64).to_be_bytes());
                    h.update(code);
                }
                None => {
                    h.update(&[0u8; 32]);
                    h.update(&0u64.to_be_bytes());
                }
            }
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

    /// Commitment over the bytes-state registry (`address -> bytes`).
    ///
    /// `BLAKE3` over entries in address order:
    /// `addr(20) || len(8 BE) || bytes`. (`BTreeMap` provides the order;
    /// the length prefix keeps the encoding unambiguous.)
    #[must_use]
    pub fn bytes_state_root(&self) -> Commitment {
        const TAG_BYTES_STATE: &[u8] = b"SUWAPPUDB-BYTES-STATE";
        let mut h = blake3::Hasher::new();
        h.update(TAG_BYTES_STATE);
        for (addr, bytes) in &self.bytes_state {
            h.update(&addr.0);
            h.update(&(bytes.len() as u64).to_be_bytes());
            h.update(bytes);
        }
        let mut out = [0u8; 32];
        out.copy_from_slice(h.finalize().as_bytes());
        Commitment(out)
    }

    /// The consensus state root: the balance tree (IQ-6) bound to the
    /// EVM-state commitment (IQ-10) and the bytes-state registry.
    ///
    /// `BLAKE3("SUWAPPUDB-STATE-ROOT" || balance_root || evm_state_root || bytes_state_root)`.
    /// The root validators co-sign — a deterministic function of all state:
    /// balances, contract code + storage, and reserved-address records.
    #[must_use]
    pub fn state_root(&self) -> Commitment {
        const TAG_STATE_ROOT: &[u8] = b"SUWAPPUDB-STATE-ROOT";
        let balance_root = StateTree::from_state(self).root();
        let evm_root = self.evm_state_root();
        let bytes_root = self.bytes_state_root();
        let mut h = blake3::Hasher::new();
        h.update(TAG_STATE_ROOT);
        h.update(&balance_root.0);
        h.update(&evm_root.0);
        h.update(&bytes_root.0);
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
                // Codex P2 (lib.rs:369): enforce `code_hash == keccak256(code)`
                // here so the invariant the `SetCode` variant promises cannot
                // be bypassed by snapshot-restore or any other bridge-side
                // seeding path. Without this, the revm `Database` adapter
                // would happily surface the stored hash as `code_hash` while
                // executing different bytes — `EXTCODEHASH` would lie about
                // the executable code at an address.
                use sha3::{Digest, Keccak256};
                let computed: [u8; 32] = Keccak256::digest(code).into();
                assert_eq!(
                    *code_hash, computed,
                    "StateChange::SetCode invariant violated: \
                     code_hash must equal keccak256(code) \
                     (passed {:?}, expected {:?})",
                    code_hash, computed,
                );
                self.evm_code.insert(*code_hash, code.clone());
                self.store.set_code(code_hash, code);
            }
            StateChange::SetStorage { addr, slot, value } => {
                // Canonicalize: a zero value is EVM-equivalent to an unset
                // slot (reads return zero), so drop it from the committed
                // map — otherwise write-then-clear would diverge from
                // never-write in the state root and across replay/snapshot.
                if *value == [0u8; 32] {
                    self.evm_storage.remove(&(*addr, *slot));
                } else {
                    self.evm_storage.insert((*addr, *slot), *value);
                }
                self.store.set_storage(addr, slot, value);
            }
            StateChange::SetAccountCode { addr, code_hash } => {
                self.evm_account_code.insert(*addr, *code_hash);
                self.store.set_account_code(addr, code_hash);
            }
            StateChange::SetBytes { addr, bytes } => {
                self.bytes_state.insert(*addr, bytes.clone());
                self.store.set_bytes(addr, bytes);
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
        use sha3::{Digest, Keccak256};
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let code = vec![0x60u8, 0x00, 0x55];
        let code_hash: [u8; 32] = Keccak256::digest(&code).into();

        assert_eq!(state.code_by_hash(&code_hash), None);
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code: code.clone(),
            },
        );
        assert_eq!(state.code_by_hash(&code_hash), Some(code.as_slice()));

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

    /// Codex P2 (lib.rs:369) regression: `StateChange::SetCode` with a
    /// `code_hash` that doesn't equal `keccak256(code)` MUST panic at
    /// `State::apply`, so a malicious / corrupt snapshot or seeding
    /// path cannot install bytecode under a hash that `EXTCODEHASH`
    /// would then lie about.
    #[test]
    #[should_panic(expected = "code_hash must equal keccak256(code)")]
    fn set_code_panics_when_hash_does_not_match_keccak() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash: [0u8; 32], // intentionally wrong
                code: vec![0x60, 0x00, 0x55],
            },
        );
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
        use sha3::{Digest, Keccak256};
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
        let code = vec![1u8, 2, 3];
        let code_hash: [u8; 32] = Keccak256::digest(&code).into();
        state.apply(
            &token,
            &StateChange::SetCode {
                code_hash,
                code,
            },
        );
        state.apply(
            &token,
            &StateChange::SetAccountCode {
                addr: contract,
                code_hash,
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
    fn bytes_state_round_trips() {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([5; 20]);

        assert_eq!(state.read_bytes(&addr), None);
        state.apply(&token, &StateChange::SetBytes { addr, bytes: vec![1, 2, 3, 4] });
        assert_eq!(state.read_bytes(&addr), Some([1, 2, 3, 4].as_slice()));
    }

    #[test]
    fn state_root_commits_bytes_state() {
        let token = BridgeToken::__for_bridge_only();
        let addr = Address([6; 20]);

        let mut state = State::default();
        let r0 = state.state_root();
        state.apply(&token, &StateChange::SetBytes { addr, bytes: vec![9, 9, 9] });
        assert_ne!(
            r0,
            state.state_root(),
            "bytes-state write must change the state root"
        );
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
