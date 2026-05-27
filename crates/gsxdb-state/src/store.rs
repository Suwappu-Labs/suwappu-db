//! `BalanceStore` — abstraction over balance storage.
//!
//! Phase-1 has two implementations:
//!
//! - [`InMemoryBalanceStore`] — `HashMap`-backed; used by tests and as the
//!   default `State` backend for unit/property testing
//! - `RocksDbBalanceStore` (slice 3) — durable, backed by the `state` column
//!   family in `RocksDB`
//!
//! Both implementations must preserve the dual-projection invariant: every
//! address's `BalanceSlot` always exposes equal `evm_balance` and
//! `move_coin_value` projections. The invariant is structural at the
//! [`BalanceSlot`] layer, but storage round-trips need their own property
//! test to confirm no implementation drops or corrupts a slot.
//!
//! The trait is intentionally infallible in phase-1. Slice 3 will introduce a
//! fallible variant alongside it when `RocksDB` IO errors enter the picture;
//! the in-memory impl will continue to be infallible by construction.

use crate::{Address, BalanceSlot};
use std::collections::HashMap;

/// Abstraction over a key/value store keyed by [`Address`] with [`BalanceSlot`]
/// values.
///
/// Implementations must satisfy two contracts:
///
/// 1. **Round-trip** — `set(a, s); get(a) == s` for every address `a` and
///    slot `s`.
/// 2. **Default-zero** — an address that has never been `set` reads as
///    [`BalanceSlot::default`] (canonical value 0).
pub trait BalanceStore {
    /// Read the slot for `addr`. Returns the default slot if `addr` has
    /// never been written.
    fn get(&self, addr: &Address) -> BalanceSlot;

    /// Write `slot` for `addr`, replacing any previous value.
    fn set(&mut self, addr: &Address, slot: BalanceSlot);

    /// Number of addresses with explicitly written slots. Implementations
    /// that don't track this efficiently may return an over-estimate, but
    /// the in-memory impl returns the exact count.
    fn len(&self) -> usize;

    /// True if no slots have been written.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Snapshot of every (addr, slot) currently in the store. Order is
    /// implementation-defined; consumers that need a canonical order
    /// (e.g. the state-tree commitment) must sort.
    ///
    /// Phase-1 returns a `Vec` for simplicity — fine for the small-state
    /// workloads property tests exercise. Production should add an
    /// iterator variant; deferred to S8 when persistence + recovery
    /// surface the question.
    fn entries(&self) -> Vec<(Address, BalanceSlot)>;

    // --- EVM contract state (code / storage / account-code) ---
    //
    // These default to no-op / empty. A store that only durably holds
    // balances (the in-memory test store) keeps EVM state in `State`'s
    // in-memory maps and needs no durable copy — the maps are the source of
    // truth and snapshots read them directly. Durable backends (redb)
    // override these so contract code + storage survive a reopen, and
    // `State::with_store` hydrates its maps from `codes()` /
    // `storage_entries()` / `account_codes()`.

    /// Persist EVM contract bytecode under its hash.
    fn set_code(&mut self, _code_hash: &[u8; 32], _code: &[u8]) {}
    /// Persist a contract storage slot. A zero `value` clears the slot.
    fn set_storage(&mut self, _addr: &Address, _slot: &[u8; 32], _value: &[u8; 32]) {}
    /// Persist an account's code-hash pointer.
    fn set_account_code(&mut self, _addr: &Address, _code_hash: &[u8; 32]) {}
    /// All durably-stored `(code_hash, code)` pairs, for hydrating `State`.
    fn codes(&self) -> Vec<([u8; 32], Vec<u8>)> {
        Vec::new()
    }
    /// All durably-stored `((addr, slot), value)` storage entries.
    fn storage_entries(&self) -> Vec<crate::EvmStorageEntry> {
        Vec::new()
    }
    /// All durably-stored `(addr, code_hash)` account-code pointers.
    fn account_codes(&self) -> Vec<(Address, [u8; 32])> {
        Vec::new()
    }
}

/// `HashMap`-backed [`BalanceStore`]. Default backend for tests and the
/// non-persistent `State` configuration.
#[derive(Debug, Default, Clone)]
pub struct InMemoryBalanceStore {
    slots: HashMap<Address, BalanceSlot>,
}

impl InMemoryBalanceStore {
    /// New empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BalanceStore for InMemoryBalanceStore {
    fn get(&self, addr: &Address) -> BalanceSlot {
        self.slots.get(addr).copied().unwrap_or_default()
    }

    fn set(&mut self, addr: &Address, slot: BalanceSlot) {
        self.slots.insert(*addr, slot);
    }

    fn len(&self) -> usize {
        self.slots.len()
    }

    fn entries(&self) -> Vec<(Address, BalanceSlot)> {
        self.slots.iter().map(|(a, s)| (*a, *s)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn empty_store_reads_default() {
        let store = InMemoryBalanceStore::new();
        assert_eq!(store.get(&addr(1)), BalanceSlot::default());
        assert!(store.is_empty());
    }

    #[test]
    fn set_then_get_round_trips() {
        let mut store = InMemoryBalanceStore::new();
        let a = addr(1);
        let slot = BalanceSlot::new(42);

        store.set(&a, slot);

        assert_eq!(store.get(&a), slot);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn set_replaces_previous_value() {
        let mut store = InMemoryBalanceStore::new();
        let a = addr(1);

        store.set(&a, BalanceSlot::new(10));
        store.set(&a, BalanceSlot::new(20));

        assert_eq!(store.get(&a).canonical(), 20);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn distinct_addresses_are_independent() {
        let mut store = InMemoryBalanceStore::new();

        store.set(&addr(1), BalanceSlot::new(100));
        store.set(&addr(2), BalanceSlot::new(200));

        assert_eq!(store.get(&addr(1)).canonical(), 100);
        assert_eq!(store.get(&addr(2)).canonical(), 200);
        assert_eq!(store.get(&addr(3)).canonical(), 0);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::balance_slot::SlotError;
    use proptest::prelude::*;

    /// Constrain addresses to a small set so the property test actually
    /// exercises repeated mutations of the same key. Truly random 20-byte
    /// addresses would almost never collide.
    fn small_address() -> impl Strategy<Value = Address> {
        (0u8..8).prop_map(|n| Address([n; 20]))
    }

    /// One typed mutation against a single address.
    #[derive(Debug, Clone, Copy)]
    enum Op {
        Deposit(Address, u128),
        Withdraw(Address, u128),
        Set(Address, u128),
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            (small_address(), any::<u128>()).prop_map(|(a, n)| Op::Deposit(a, n)),
            (small_address(), any::<u128>()).prop_map(|(a, n)| Op::Withdraw(a, n)),
            (small_address(), any::<u128>()).prop_map(|(a, n)| Op::Set(a, n)),
        ]
    }

    /// Apply one op to a store. Returns `Some` slot error if a deposit/withdraw
    /// was rejected (no state change). `Set` is always applied.
    fn apply<S: BalanceStore>(store: &mut S, op: Op) -> Option<SlotError> {
        match op {
            Op::Deposit(a, n) => {
                let mut slot = store.get(&a);
                match slot.deposit(n) {
                    Ok(()) => {
                        store.set(&a, slot);
                        None
                    }
                    Err(e) => Some(e),
                }
            }
            Op::Withdraw(a, n) => {
                let mut slot = store.get(&a);
                match slot.withdraw(n) {
                    Ok(()) => {
                        store.set(&a, slot);
                        None
                    }
                    Err(e) => Some(e),
                }
            }
            Op::Set(a, n) => {
                store.set(&a, BalanceSlot::new(n));
                None
            }
        }
    }

    proptest! {
        /// **Storage-level dual-projection invariant.**
        /// After any sequence of ops across many addresses, every address's
        /// projections agree (EVM == Move == canonical).
        #[test]
        fn store_preserves_dual_projection(
            ops in proptest::collection::vec(op_strategy(), 0..128),
        ) {
            let mut store = InMemoryBalanceStore::new();
            let mut touched: Vec<Address> = Vec::new();

            for op in ops {
                let touched_addr = match op {
                    Op::Deposit(a, _) | Op::Withdraw(a, _) | Op::Set(a, _) => a,
                };
                let _ = apply(&mut store, op);

                // After every op, every previously-touched address holds a
                // slot whose projections agree.
                if !touched.contains(&touched_addr) {
                    touched.push(touched_addr);
                }
                for a in &touched {
                    let slot = store.get(a);
                    prop_assert_eq!(
                        slot.evm_balance().to_u128(),
                        slot.move_coin_value().to_u128()
                    );
                    prop_assert_eq!(slot.evm_balance().to_u128(), slot.canonical());
                }
            }
        }

        /// **Default-zero contract.** Addresses that were never written
        /// always read as default, regardless of operations on other addrs.
        #[test]
        fn untouched_addresses_read_default(
            ops in proptest::collection::vec(op_strategy(), 0..64),
            never_touched_seed in 200u8..=255,
        ) {
            let mut store = InMemoryBalanceStore::new();
            // Pick an address outside the small_address() range so it's
            // guaranteed not to appear in `ops`.
            let untouched = Address([never_touched_seed; 20]);

            for op in ops {
                let _ = apply(&mut store, op);
                prop_assert_eq!(store.get(&untouched), BalanceSlot::default());
            }
        }

        /// **Determinism.** Two stores starting empty and receiving the same
        /// op sequence end in equivalent states (same value at every touched
        /// address, same len).
        #[test]
        fn op_replay_is_deterministic(
            ops in proptest::collection::vec(op_strategy(), 0..64),
        ) {
            let mut a = InMemoryBalanceStore::new();
            let mut b = InMemoryBalanceStore::new();

            for op in ops.iter().copied() {
                let _ = apply(&mut a, op);
                let _ = apply(&mut b, op);
            }

            prop_assert_eq!(a.len(), b.len());
            for op in &ops {
                let addr = match op {
                    Op::Deposit(x, _) | Op::Withdraw(x, _) | Op::Set(x, _) => x,
                };
                prop_assert_eq!(a.get(addr), b.get(addr));
            }
        }
    }
}
