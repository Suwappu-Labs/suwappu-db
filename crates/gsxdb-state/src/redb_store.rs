//! `RedbBalanceStore` — durable [`BalanceStore`] backed by `redb`.
//!
//! Phase-1 dev / CI backend. See `docs/iq/IQ-1-redb-vs-rocksdb.md` for the
//! decision to use redb in development while keeping `RocksDB` as the
//! production target. Both implementations satisfy the same
//! [`BalanceStore`] trait, so production swap is mechanical.
//!
//! # Tables
//!
//! Five tables, mirroring the original 5-column-family design from the
//! Phase-1 spec:
//!
//! | Table             | Used by      | Slice |
//! |-------------------|--------------|-------|
//! | `state`           | this store   | S2.3  |
//! | `aggregates`      | _reserved_   | S2+   |
//! | `evm_storage`     | _reserved_   | S3    |
//! | `evm_nonces`      | _reserved_   | S3    |
//! | `move_resources`  | _reserved_   | S3    |
//!
//! All five are materialised at [`RedbBalanceStore::open`] so future slices
//! can write to them without re-opening the database.
//!
//! # Encoding
//!
//! - **Keys** — raw 20-byte address (`Address::0`)
//! - **Values** — canonical balance as 16-byte big-endian `u128`
//!
//! # Failure model
//!
//! The phase-1 [`BalanceStore`] trait is infallible (see `store.rs`). This
//! impl satisfies it by panicking on redb errors with explicit messages.
//! Real fault tolerance lands in S8 when the fallible trait variant arrives.

use crate::store::BalanceStore;
use crate::{Address, BalanceSlot};
use redb::{Database, ReadableTableMetadata, TableDefinition};
use std::path::Path;
use std::sync::Arc;

/// Table holding `Address` → `BalanceSlot`.
pub const TABLE_STATE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("state");
/// Table reserved for cross-VM aggregates (S2 follow-up).
pub const TABLE_AGGREGATES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("aggregates");
/// Table reserved for EVM contract storage slots (S3).
pub const TABLE_EVM_STORAGE: TableDefinition<&[u8], &[u8]> = TableDefinition::new("evm_storage");
/// Table reserved for EVM account nonces (S3).
pub const TABLE_EVM_NONCES: TableDefinition<&[u8], &[u8]> = TableDefinition::new("evm_nonces");
/// Table reserved for Move resource trees (S3).
pub const TABLE_MOVE_RESOURCES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("move_resources");

/// All tables the database is opened with. Order is irrelevant.
pub const ALL_TABLES: &[TableDefinition<&[u8], &[u8]>] = &[
    TABLE_STATE,
    TABLE_AGGREGATES,
    TABLE_EVM_STORAGE,
    TABLE_EVM_NONCES,
    TABLE_MOVE_RESOURCES,
];

const VALUE_LEN: usize = 16; // u128 big-endian

/// `redb`-backed [`BalanceStore`].
///
/// Cheap to clone — internally an [`Arc`] handle to the database.
#[derive(Clone)]
pub struct RedbBalanceStore {
    db: Arc<Database>,
}

impl std::fmt::Debug for RedbBalanceStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedbBalanceStore").finish_non_exhaustive()
    }
}

impl RedbBalanceStore {
    /// Open or create a redb database at `path` with all five tables.
    /// Idempotent: re-opening an existing database is fine.
    ///
    /// # Errors
    ///
    /// Returns the underlying `redb::Error` family if the file cannot be
    /// opened or a table cannot be created. Boxed because the `redb::Error`
    /// enum is large (~160 bytes) and we don't want every callsite to pay
    /// for that on the success path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Box<redb::Error>> {
        let db = Database::create(path).map_err(|e| Box::new(redb::Error::from(e)))?;

        // Materialise every table so callers don't trip over a missing
        // table on first read of a reserved slot.
        let txn = db
            .begin_write()
            .map_err(|e| Box::new(redb::Error::from(e)))?;
        for table_def in ALL_TABLES {
            let _ = txn
                .open_table(*table_def)
                .map_err(|e| Box::new(redb::Error::from(e)))?;
        }
        txn.commit().map_err(|e| Box::new(redb::Error::from(e)))?;

        Ok(Self { db: Arc::new(db) })
    }

    fn encode_value(slot: BalanceSlot) -> [u8; VALUE_LEN] {
        slot.canonical().to_be_bytes()
    }

    fn decode_value(bytes: &[u8]) -> BalanceSlot {
        assert_eq!(
            bytes.len(),
            VALUE_LEN,
            "balance value: expected {VALUE_LEN} bytes, got {}",
            bytes.len()
        );
        let mut buf = [0u8; VALUE_LEN];
        buf.copy_from_slice(bytes);
        BalanceSlot::new(u128::from_be_bytes(buf))
    }
}

impl BalanceStore for RedbBalanceStore {
    fn get(&self, addr: &Address) -> BalanceSlot {
        let txn = self
            .db
            .begin_read()
            .expect("RedbBalanceStore::get: begin_read failed");
        let table = txn
            .open_table(TABLE_STATE)
            .expect("RedbBalanceStore::get: open_table(state) failed");
        match table.get(addr.0.as_slice()) {
            Ok(Some(v)) => Self::decode_value(v.value()),
            Ok(None) => BalanceSlot::default(),
            Err(e) => panic!("RedbBalanceStore::get failed for {:?}: {e}", addr.0),
        }
    }

    fn set(&mut self, addr: &Address, slot: BalanceSlot) {
        let txn = self
            .db
            .begin_write()
            .expect("RedbBalanceStore::set: begin_write failed");
        {
            let mut table = txn
                .open_table(TABLE_STATE)
                .expect("RedbBalanceStore::set: open_table(state) failed");
            let value = Self::encode_value(slot);
            table
                .insert(addr.0.as_slice(), value.as_slice())
                .unwrap_or_else(|e| {
                    panic!("RedbBalanceStore::set insert failed for {:?}: {e}", addr.0)
                });
        }
        txn.commit().expect("RedbBalanceStore::set: commit failed");
    }

    fn len(&self) -> usize {
        let txn = self
            .db
            .begin_read()
            .expect("RedbBalanceStore::len: begin_read failed");
        let table = txn
            .open_table(TABLE_STATE)
            .expect("RedbBalanceStore::len: open_table(state) failed");
        usize::try_from(
            table
                .len()
                .expect("RedbBalanceStore::len: table.len failed"),
        )
        .expect("len exceeds usize")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fresh_store() -> (RedbBalanceStore, TempDir) {
        let dir = TempDir::new().expect("tempdir");
        let store = RedbBalanceStore::open(dir.path().join("state.redb")).expect("open");
        (store, dir)
    }

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    #[test]
    fn opens_with_all_five_tables() {
        use redb::TableHandle;

        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.redb");
        let _ = RedbBalanceStore::open(&path).unwrap();

        // Re-open and confirm every reserved table is present.
        let db = Database::open(&path).unwrap();
        let txn = db.begin_read().unwrap();
        for table_def in ALL_TABLES {
            let _ = txn
                .open_table(*table_def)
                .unwrap_or_else(|e| panic!("missing table {}: {e}", table_def.name()));
        }
    }

    #[test]
    fn empty_store_reads_default() {
        let (store, _dir) = fresh_store();
        assert_eq!(store.get(&addr(1)), BalanceSlot::default());
        assert!(store.is_empty());
    }

    #[test]
    fn set_then_get_round_trips() {
        let (mut store, _dir) = fresh_store();
        let a = addr(1);
        let slot = BalanceSlot::new(42);

        store.set(&a, slot);

        assert_eq!(store.get(&a), slot);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn persistence_across_reopen() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("state.redb");
        let a = addr(7);
        let slot = BalanceSlot::new(12345);

        {
            let mut store = RedbBalanceStore::open(&path).unwrap();
            store.set(&a, slot);
        }

        let store = RedbBalanceStore::open(&path).unwrap();
        assert_eq!(store.get(&a), slot);
    }

    #[test]
    fn distinct_addresses_are_independent() {
        let (mut store, _dir) = fresh_store();

        store.set(&addr(1), BalanceSlot::new(100));
        store.set(&addr(2), BalanceSlot::new(200));

        assert_eq!(store.get(&addr(1)).canonical(), 100);
        assert_eq!(store.get(&addr(2)).canonical(), 200);
        assert_eq!(store.get(&addr(3)).canonical(), 0);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn extreme_values_round_trip() {
        let (mut store, _dir) = fresh_store();

        store.set(&addr(0), BalanceSlot::new(0));
        store.set(&addr(1), BalanceSlot::new(u128::MAX));
        store.set(&addr(2), BalanceSlot::new(u128::MAX / 2));

        assert_eq!(store.get(&addr(0)).canonical(), 0);
        assert_eq!(store.get(&addr(1)).canonical(), u128::MAX);
        assert_eq!(store.get(&addr(2)).canonical(), u128::MAX / 2);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use crate::balance_slot::SlotError;
    use proptest::prelude::*;
    use tempfile::TempDir;

    fn small_address() -> impl Strategy<Value = Address> {
        (0u8..8).prop_map(|n| Address([n; 20]))
    }

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
        // redb operations open a real on-disk db per case. Keep the case
        // count modest in the default profile; use PROPTEST_CASES=10000 in
        // release mode for the exit-gate verification.
        #![proptest_config(ProptestConfig {
            cases: 64,
            .. ProptestConfig::default()
        })]

        /// **Persistent dual-projection invariant** — S2 EXIT GATE.
        ///
        /// Same property as the in-memory store but exercised against the
        /// persistent backend. Encoding, decoding, and table-open bugs all
        /// surface here.
        #[test]
        fn redb_preserves_dual_projection(
            ops in proptest::collection::vec(op_strategy(), 0..32),
        ) {
            let dir = TempDir::new().unwrap();
            let mut store = RedbBalanceStore::open(dir.path().join("state.redb")).unwrap();
            let mut touched: Vec<Address> = Vec::new();

            for op in ops {
                let touched_addr = match op {
                    Op::Deposit(a, _) | Op::Withdraw(a, _) | Op::Set(a, _) => a,
                };
                let _ = apply(&mut store, op);

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

        /// **Backend equivalence.** For any op sequence, the in-memory store
        /// and the redb store agree on the final value at every touched
        /// address. Validates the encoding round-trip end-to-end.
        #[test]
        fn redb_matches_in_memory(
            ops in proptest::collection::vec(op_strategy(), 0..32),
        ) {
            let dir = TempDir::new().unwrap();
            let mut rocks = RedbBalanceStore::open(dir.path().join("state.redb")).unwrap();
            let mut mem = crate::InMemoryBalanceStore::new();

            for op in ops.iter().copied() {
                let _ = apply(&mut rocks, op);
                let _ = apply(&mut mem, op);
            }

            for op in &ops {
                let addr = match op {
                    Op::Deposit(x, _) | Op::Withdraw(x, _) | Op::Set(x, _) => x,
                };
                prop_assert_eq!(rocks.get(addr), mem.get(addr));
            }
        }
    }
}
