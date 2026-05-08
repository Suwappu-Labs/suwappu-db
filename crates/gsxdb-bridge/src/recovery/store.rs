//! `BlockStore` trait + in-memory impl.
//!
//! Phase-1 ships the in-memory backend; a redb-backed durable store
//! lands in S8.5 alongside production deployment. Same dev/prod
//! pattern as IQ-1 (redb in dev / `RocksDB` in prod for the state
//! store).

use super::block::{Block, BlockHash};
use std::collections::{BTreeMap, HashMap};

/// Append-only block storage.
pub trait BlockStore {
    /// Append `block`. Stores the block keyed by its hash and indexed
    /// by its height.
    fn put(&mut self, block: Block);

    /// Lookup by hash.
    fn get_by_hash(&self, hash: &BlockHash) -> Option<Block>;

    /// Lookup by logical height.
    fn get_by_height(&self, height: u64) -> Option<Block>;

    /// Latest block by height, if any.
    fn latest(&self) -> Option<Block>;

    /// Number of blocks.
    fn len(&self) -> usize;

    /// `true` iff no blocks stored.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Iterate every block in height order, starting at `from`
    /// inclusive. Phase-1 returns a `Vec` for simplicity.
    fn iter_from(&self, from: u64) -> Vec<Block>;
}

/// In-memory block store. Cheap; loses everything on drop.
#[derive(Debug, Default, Clone)]
pub struct InMemoryBlockStore {
    by_hash: HashMap<BlockHash, Block>,
    by_height: BTreeMap<u64, BlockHash>,
}

impl InMemoryBlockStore {
    /// New empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl BlockStore for InMemoryBlockStore {
    fn put(&mut self, block: Block) {
        let hash = block.hash();
        self.by_height.insert(block.height, hash);
        self.by_hash.insert(hash, block);
    }

    fn get_by_hash(&self, hash: &BlockHash) -> Option<Block> {
        self.by_hash.get(hash).cloned()
    }

    fn get_by_height(&self, height: u64) -> Option<Block> {
        self.by_height
            .get(&height)
            .and_then(|h| self.by_hash.get(h).cloned())
    }

    fn latest(&self) -> Option<Block> {
        self.by_height
            .iter()
            .next_back()
            .and_then(|(_, h)| self.by_hash.get(h).cloned())
    }

    fn len(&self) -> usize {
        self.by_hash.len()
    }

    fn iter_from(&self, from: u64) -> Vec<Block> {
        self.by_height
            .range(from..)
            .filter_map(|(_, h)| self.by_hash.get(h).cloned())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::block::GENESIS_PARENT;
    use crate::Intent;
    use gsxdb_state::{Address, Commitment};

    fn addr(byte: u8) -> Address {
        Address([byte; 20])
    }

    fn block(height: u64, parent: BlockHash, state_root: Commitment) -> Block {
        Block {
            height,
            parent,
            state_root,
            intents: vec![Intent::Transfer {
                from: addr(0),
                to: addr(1),
                amount: u128::from(height),
            }],
        }
    }

    #[test]
    fn empty_store_is_empty() {
        let s = InMemoryBlockStore::new();
        assert!(s.is_empty());
        assert_eq!(s.len(), 0);
        assert!(s.latest().is_none());
        assert!(s.get_by_height(0).is_none());
    }

    #[test]
    fn put_and_lookup_round_trip() {
        let mut s = InMemoryBlockStore::new();
        let b = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let hash = b.hash();
        s.put(b.clone());

        assert_eq!(s.len(), 1);
        assert_eq!(s.get_by_hash(&hash), Some(b.clone()));
        assert_eq!(s.get_by_height(0), Some(b.clone()));
        assert_eq!(s.latest(), Some(b));
    }

    #[test]
    fn latest_tracks_highest_height() {
        let mut s = InMemoryBlockStore::new();
        let b0 = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let b1 = block(1, b0.hash(), Commitment([2; 32]));
        let b2 = block(2, b1.hash(), Commitment([3; 32]));
        s.put(b1.clone()); // out of order
        s.put(b2.clone());
        s.put(b0.clone());

        assert_eq!(s.latest(), Some(b2));
        assert_eq!(s.get_by_height(0), Some(b0));
        assert_eq!(s.get_by_height(1), Some(b1));
    }

    #[test]
    fn iter_from_filters_by_height() {
        let mut s = InMemoryBlockStore::new();
        let b0 = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let b1 = block(1, b0.hash(), Commitment([2; 32]));
        let b2 = block(2, b1.hash(), Commitment([3; 32]));
        s.put(b0);
        s.put(b1.clone());
        s.put(b2.clone());

        let from_1 = s.iter_from(1);
        assert_eq!(from_1.len(), 2);
        assert_eq!(from_1[0], b1);
        assert_eq!(from_1[1], b2);

        let from_5 = s.iter_from(5);
        assert!(from_5.is_empty());
    }
}
