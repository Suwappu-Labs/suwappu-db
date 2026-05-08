//! Per-txn read/write set tracking + the OCC validator.
//!
//! In Block-STM, a transaction's read set records *what version was
//! observed at each address*, and the write set records *what value
//! the txn intends to install at each address*. At validation time,
//! we ask: for every observed `(addr, source)` in the read set, is
//! `source` still the highest version below `my_idx` in the
//! [`MvStore`]? If not, the read is stale and the txn must re-execute.

use crate::occ::mv_store::{MvStore, ReadSource, TxnIdx};
use gsxdb_state::{Address, BalanceSlot};

/// One observed read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadEntry {
    /// Address read.
    pub addr: Address,
    /// Source the read resolved against at observation time.
    pub source: ReadSource,
}

/// One intended write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteEntry {
    /// Address written.
    pub addr: Address,
    /// New value.
    pub value: BalanceSlot,
}

/// A speculatively executed transaction's recorded effects.
///
/// The validator consumes this; if validation passes, the writes were
/// already published to the [`MvStore`] during execution and stay
/// there. If validation fails, [`MvStore::clear_writes`] is called for
/// `idx` and the txn re-executes.
#[derive(Debug, Clone, Default)]
pub struct Txn {
    /// Tx position in the block. Drives version ordering.
    pub idx: TxnIdx,
    /// Reads observed during execution.
    pub read_set: Vec<ReadEntry>,
    /// Writes published during execution.
    pub write_set: Vec<WriteEntry>,
    /// Whether the txn's domain logic returned an error (e.g.
    /// insufficient balance). Rejected txns commit zero writes.
    pub rejected: Option<crate::RejectReason>,
}

/// OCC validator. Stateless; holds no data of its own.
#[derive(Debug, Default, Clone, Copy)]
pub struct Validator;

impl Validator {
    /// `true` iff every observed read in the txn's read set is still
    /// the highest visible version at `txn.idx`. The check is a pure
    /// function of the txn and the current [`MvStore`].
    #[must_use]
    pub fn is_valid(self, txn: &Txn, mv: &MvStore) -> bool {
        for entry in &txn.read_set {
            let observed = entry.source;
            let current = mv.highest_writer_below(&entry.addr, txn.idx);
            let still_consistent = match (observed, current) {
                // Read against snapshot is valid only if no earlier txn
                // has since written to `addr`.
                (ReadSource::Snapshot, None) => true,
                (ReadSource::Snapshot, Some(_)) => false,
                // Read against version `v` is valid only if `v` is
                // still the highest writer below my index.
                (ReadSource::Version(v), Some(c)) => v == c,
                // We observed a version but no version exists now —
                // means it was cleared by re-execution.
                (ReadSource::Version(_), None) => false,
            };
            if !still_consistent {
                return false;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::Address;

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn entry(a: Address, src: ReadSource) -> ReadEntry {
        ReadEntry { addr: a, source: src }
    }

    #[test]
    fn empty_read_set_is_valid() {
        let txn = Txn {
            idx: 0,
            ..Default::default()
        };
        let mv = MvStore::new();
        assert!(Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn snapshot_read_valid_when_no_prior_writer() {
        let mv = MvStore::new();
        let txn = Txn {
            idx: 5,
            read_set: vec![entry(addr(1), ReadSource::Snapshot)],
            ..Default::default()
        };
        assert!(Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn snapshot_read_invalid_when_earlier_writer_exists() {
        let mv = MvStore::new();
        // An earlier txn (idx 2) wrote to addr(1) AFTER our txn at 5
        // first read it as snapshot. We must re-execute.
        mv.write(addr(1), BalanceSlot::new(99), 2);
        let txn = Txn {
            idx: 5,
            read_set: vec![entry(addr(1), ReadSource::Snapshot)],
            ..Default::default()
        };
        assert!(!Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn version_read_valid_when_version_unchanged() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 2);

        let txn = Txn {
            idx: 5,
            read_set: vec![entry(addr(1), ReadSource::Version(2))],
            ..Default::default()
        };
        assert!(Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn version_read_invalid_when_later_writer_supersedes() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 2);
        mv.write(addr(1), BalanceSlot::new(20), 4); // supersedes from txn 5's POV

        let txn = Txn {
            idx: 5,
            read_set: vec![entry(addr(1), ReadSource::Version(2))],
            ..Default::default()
        };
        assert!(!Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn version_read_invalid_when_version_cleared() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 2);
        let txn = Txn {
            idx: 5,
            read_set: vec![entry(addr(1), ReadSource::Version(2))],
            ..Default::default()
        };
        assert!(Validator.is_valid(&txn, &mv));

        mv.clear_writes(2);
        assert!(!Validator.is_valid(&txn, &mv));
    }

    #[test]
    fn idempotent_repeated_validation() {
        let mv = MvStore::new();
        mv.write(addr(1), BalanceSlot::new(10), 1);
        let txn = Txn {
            idx: 3,
            read_set: vec![entry(addr(1), ReadSource::Version(1))],
            ..Default::default()
        };
        // Calling twice with no MV mutation in between yields the same answer.
        let a = Validator.is_valid(&txn, &mv);
        let b = Validator.is_valid(&txn, &mv);
        assert_eq!(a, b);
        assert!(a);
    }
}
