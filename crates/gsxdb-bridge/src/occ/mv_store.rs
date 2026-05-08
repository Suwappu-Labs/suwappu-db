//! Multi-version balance store.
//!
//! Holds speculative writes per `(Address, TxnIdx)`. A read at index
//! `my_idx` returns the highest-versioned write strictly below
//! `my_idx`, or — if no such write exists — the underlying canonical
//! [`State`] balance.
//!
//! # Concurrency
//!
//! The store is wrapped in per-address locks via `parking_lot`-style
//! `Mutex` (we use `std::sync::Mutex` here to avoid the dep). rayon
//! workers acquire locks per address-touched per txn; lock duration is
//! short (single read/write to a `BTreeMap`), so contention is minimal
//! at the workloads phase-1 cares about.
//!
//! # Phase-1 simplifications
//!
//! - Only `BalanceSlot` is versioned. Storage slots, nonces, and Move
//!   resources are out of scope (S5/S6).
//! - The "snapshot" of the underlying [`State`] is taken at
//!   construction; any concurrent mutation of `State` during block
//!   execution is undefined behaviour. The block executor enforces
//!   exclusive `&mut State` access.

use gsxdb_state::{Address, BalanceSlot, State};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// Index of a transaction within a block. Block-STM's ordering scalar.
pub type TxnIdx = usize;

/// What an [`MvStore::read`] resolved against. Used by the validator
/// to detect read-set invalidations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadSource {
    /// Read returned the value written by `idx`. The validator
    /// invalidates the reading txn if any earlier-committed txn at
    /// `idx < other < reader_idx` writes to the same address before
    /// the reader retries.
    Version(TxnIdx),
    /// Read fell through to the canonical [`State`] snapshot. Validator
    /// invalidates the reader if any txn with `other < reader_idx`
    /// wrote to that address (since the snapshot read should have
    /// observed that write).
    Snapshot,
}

/// Versioned balance store. Construct one per block.
pub struct MvStore {
    /// Per-address sorted version map. `BTreeMap` lets us pull the
    /// highest entry below a given index in O(log n).
    versions: Mutex<HashMap<Address, BTreeMap<TxnIdx, BalanceSlot>>>,
    /// Snapshot of canonical state at block start. Reads fall through
    /// here if no version exists.
    snapshot: HashMap<Address, BalanceSlot>,
}

impl MvStore {
    /// New store wrapping a snapshot of the underlying state. The
    /// snapshot is logical: we record the addresses we expect to touch
    /// (lazily, on first read) and read them through to the underlying
    /// state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            versions: Mutex::new(HashMap::new()),
            snapshot: HashMap::new(),
        }
    }

    /// Read the value visible to `my_idx`. Returns the value and the
    /// source — the latter is what the validator uses to detect
    /// invalidations.
    ///
    /// Reads from the canonical state on cold-cache fall through.
    pub fn read(&self, state: &State, addr: &Address, my_idx: TxnIdx) -> (BalanceSlot, ReadSource) {
        let versions = self.versions.lock().expect("MvStore versions lock");
        if let Some(per_addr) = versions.get(addr) {
            // Highest version strictly less than `my_idx`.
            if let Some((&v_idx, &slot)) = per_addr.range(..my_idx).next_back() {
                return (slot, ReadSource::Version(v_idx));
            }
        }
        drop(versions);
        // Cold-cache fall through to canonical state.
        (state.slot_of(addr), ReadSource::Snapshot)
    }

    /// Insert (or overwrite) a versioned write at `my_idx`.
    pub fn write(&self, addr: Address, slot: BalanceSlot, my_idx: TxnIdx) {
        let mut versions = self.versions.lock().expect("MvStore versions lock");
        versions.entry(addr).or_default().insert(my_idx, slot);
    }

    /// Drop all writes belonging to `idx`. Called when a txn is being
    /// re-executed.
    pub fn clear_writes(&self, idx: TxnIdx) {
        let mut versions = self.versions.lock().expect("MvStore versions lock");
        for per_addr in versions.values_mut() {
            per_addr.remove(&idx);
        }
        versions.retain(|_, m| !m.is_empty());
    }

    /// Highest versioned writer for `addr` strictly less than `bound`.
    /// Used by the validator to check if a recorded read is stale.
    pub fn highest_writer_below(&self, addr: &Address, bound: TxnIdx) -> Option<TxnIdx> {
        let versions = self.versions.lock().expect("MvStore versions lock");
        versions
            .get(addr)
            .and_then(|per_addr| per_addr.range(..bound).next_back().map(|(&k, _)| k))
    }

    /// Walk the store at "highest version per address" and return one
    /// entry per touched address. Used at consolidation time to write
    /// the post-block state through the bridge.
    pub fn finalise(self) -> Vec<(Address, BalanceSlot)> {
        let versions = self.versions.into_inner().expect("MvStore versions lock");
        versions
            .into_iter()
            .filter_map(|(addr, per_addr)| {
                per_addr
                    .into_iter()
                    .next_back()
                    .map(|(_, slot)| (addr, slot))
            })
            .collect()
    }

    /// Mark `addr` as "snapshot-observed" — used if we ever want to
    /// distinguish "read but not written" from "never touched."
    /// Currently unused; reserved for the validator's cold-cache path.
    #[allow(dead_code)]
    fn note_snapshot(&mut self, addr: Address, slot: BalanceSlot) {
        self.snapshot.insert(addr, slot);
    }
}

impl Default for MvStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for MvStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MvStore").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::{Balance, BridgeToken, StateChange};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn seeded(addr_in: Address, amount: u128) -> State {
        let mut state = State::default();
        let token = BridgeToken::__for_bridge_only();
        state.apply(
            &token,
            &StateChange::SetBalance {
                addr: addr_in,
                to: Balance(amount),
            },
        );
        state
    }

    #[test]
    fn empty_store_falls_through_to_snapshot() {
        let state = seeded(addr(1), 100);
        let mv = MvStore::new();
        let (slot, src) = mv.read(&state, &addr(1), 0);
        assert_eq!(slot.canonical(), 100);
        assert_eq!(src, ReadSource::Snapshot);
    }

    #[test]
    fn write_visible_to_later_index() {
        let state = State::default();
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(42), 0);

        let (slot, src) = mv.read(&state, &addr(1), 1);
        assert_eq!(slot.canonical(), 42);
        assert_eq!(src, ReadSource::Version(0));
    }

    #[test]
    fn write_invisible_to_same_or_earlier_index() {
        let state = State::default();
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(42), 5);

        // Same index: must not see one's own write through `read`.
        let (slot, src) = mv.read(&state, &addr(1), 5);
        assert_eq!(slot.canonical(), 0);
        assert_eq!(src, ReadSource::Snapshot);

        // Earlier index: ditto.
        let (slot, src) = mv.read(&state, &addr(1), 2);
        assert_eq!(slot.canonical(), 0);
        assert_eq!(src, ReadSource::Snapshot);
    }

    #[test]
    fn read_returns_highest_visible_version() {
        let state = State::default();
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 1);
        mv.write(addr(1), BalanceSlot::new(20), 3);
        mv.write(addr(1), BalanceSlot::new(30), 5);

        // Reader at 4 sees version 3 (=20), not 5.
        let (slot, src) = mv.read(&state, &addr(1), 4);
        assert_eq!(slot.canonical(), 20);
        assert_eq!(src, ReadSource::Version(3));

        // Reader at 6 sees version 5 (=30).
        let (slot, src) = mv.read(&state, &addr(1), 6);
        assert_eq!(slot.canonical(), 30);
        assert_eq!(src, ReadSource::Version(5));
    }

    #[test]
    fn clear_writes_removes_only_targeted_index() {
        let state = State::default();
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 1);
        mv.write(addr(1), BalanceSlot::new(20), 2);
        mv.write(addr(1), BalanceSlot::new(30), 3);

        mv.clear_writes(2);

        // Reader at 5 now sees version 3 (=30); version 2 is gone.
        let (slot, src) = mv.read(&state, &addr(1), 5);
        assert_eq!(slot.canonical(), 30);
        assert_eq!(src, ReadSource::Version(3));

        // Reader at 3 sees version 1 (=10), not the cleared 2.
        let (slot, src) = mv.read(&state, &addr(1), 3);
        assert_eq!(slot.canonical(), 10);
        assert_eq!(src, ReadSource::Version(1));
    }

    #[test]
    fn highest_writer_below_finds_correct_predecessor() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 1);
        mv.write(addr(1), BalanceSlot::new(20), 4);
        mv.write(addr(1), BalanceSlot::new(30), 7);

        assert_eq!(mv.highest_writer_below(&addr(1), 5), Some(4));
        assert_eq!(mv.highest_writer_below(&addr(1), 4), Some(1));
        assert_eq!(mv.highest_writer_below(&addr(1), 1), None);
        assert_eq!(mv.highest_writer_below(&addr(2), 999), None);
    }

    #[test]
    fn finalise_returns_one_entry_per_touched_address() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 0);
        mv.write(addr(1), BalanceSlot::new(20), 5);
        mv.write(addr(2), BalanceSlot::new(99), 3);

        let mut out = mv.finalise();
        out.sort_by_key(|(a, _)| a.0);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, addr(1));
        assert_eq!(out[0].1.canonical(), 20); // highest version wins
        assert_eq!(out[1].0, addr(2));
        assert_eq!(out[1].1.canonical(), 99);
    }
}
