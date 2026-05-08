//! `BlockStore` trait + in-memory impl.
//!
//! Phase-1 ships the in-memory backend; a redb-backed durable store
//! lands in S8.5 alongside production deployment. Same dev/prod
//! pattern as IQ-1 (redb in dev / `RocksDB` in prod for the state
//! store).

use super::block::{Block, BlockHash};
use crate::Intent;
use gsxdb_state::{Address, Commitment};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const BLOCKS_BY_HASH: TableDefinition<[u8; 32], &[u8]> = TableDefinition::new("blocks_by_hash");
const HEIGHT_TO_HASH: TableDefinition<u64, [u8; 32]> = TableDefinition::new("height_to_hash");
const BLOCK_ENCODING_VERSION: u8 = 1;

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

/// redb-backed persistent block store.
#[derive(Debug)]
pub struct RedbBlockStore {
    db: Database,
}

impl RedbBlockStore {
    /// Open or create a redb-backed block store at `path`.
    ///
    /// # Errors
    ///
    /// Returns an error if the database cannot be opened/created.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, redb::DatabaseError> {
        let db = if path.as_ref().exists() {
            Database::open(path)?
        } else {
            Database::create(path)?
        };

        let write_txn = db.begin_write().expect("redb begin_write");
        {
            write_txn
                .open_table(BLOCKS_BY_HASH)
                .expect("open blocks_by_hash table");
            write_txn
                .open_table(HEIGHT_TO_HASH)
                .expect("open height_to_hash table");
        }
        write_txn.commit().expect("commit table initialization");

        Ok(Self { db })
    }
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

impl BlockStore for RedbBlockStore {
    fn put(&mut self, block: Block) {
        let hash = block.hash();
        let encoded = encode_block(&block);

        let write_txn = self.db.begin_write().expect("redb begin_write");
        {
            let mut blocks = write_txn
                .open_table(BLOCKS_BY_HASH)
                .expect("open blocks_by_hash table");
            blocks
                .insert(hash.0, encoded.as_slice())
                .expect("insert block bytes by hash");
        }
        {
            let mut heights = write_txn
                .open_table(HEIGHT_TO_HASH)
                .expect("open height_to_hash table");
            heights
                .insert(block.height, hash.0)
                .expect("insert hash by height");
        }
        write_txn.commit().expect("commit block put");
    }

    fn get_by_hash(&self, hash: &BlockHash) -> Option<Block> {
        let read_txn = self.db.begin_read().expect("redb begin_read");
        let table = read_txn
            .open_table(BLOCKS_BY_HASH)
            .expect("open blocks_by_hash table");
        table
            .get(hash.0)
            .expect("read block by hash")
            .and_then(|v| decode_block(v.value()))
    }

    fn get_by_height(&self, height: u64) -> Option<Block> {
        let read_txn = self.db.begin_read().expect("redb begin_read");
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .expect("open height_to_hash table");
        let hash = heights
            .get(height)
            .expect("read hash by height")?
            .value();
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .expect("open blocks_by_hash table");
        blocks
            .get(hash)
            .expect("read block by hash")
            .and_then(|v| decode_block(v.value()))
    }

    fn latest(&self) -> Option<Block> {
        let read_txn = self.db.begin_read().expect("redb begin_read");
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .expect("open height_to_hash table");
        let (_, hash) = heights.last().expect("get latest height entry")?;
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .expect("open blocks_by_hash table");
        blocks
            .get(hash.value())
            .expect("read latest block by hash")
            .and_then(|v| decode_block(v.value()))
    }

    fn len(&self) -> usize {
        let read_txn = self.db.begin_read().expect("redb begin_read");
        let table = read_txn
            .open_table(BLOCKS_BY_HASH)
            .expect("open blocks_by_hash table");
        table
            .len()
            .expect("read block table len")
            .try_into()
            .unwrap_or(usize::MAX)
    }

    fn iter_from(&self, from: u64) -> Vec<Block> {
        let read_txn = self.db.begin_read().expect("redb begin_read");
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .expect("open height_to_hash table");
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .expect("open blocks_by_hash table");

        heights
            .range(from..)
            .expect("iterate heights")
            .filter_map(Result::ok)
            .filter_map(|(_, h)| blocks.get(h.value()).expect("read iter block"))
            .filter_map(|v| decode_block(v.value()))
            .collect()
    }
}

fn encode_block(block: &Block) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BLOCK_ENCODING_VERSION);
    out.extend_from_slice(&block.height.to_be_bytes());
    out.extend_from_slice(&block.parent.0);
    out.extend_from_slice(&block.state_root.0);
    out.extend_from_slice(&u32::try_from(block.intents.len()).unwrap_or(u32::MAX).to_be_bytes());
    for intent in &block.intents {
        encode_intent(intent, &mut out);
    }
    out
}

fn decode_block(bytes: &[u8]) -> Option<Block> {
    let mut cur = Cursor::new(bytes);
    let version = cur.byte()?;
    if version != BLOCK_ENCODING_VERSION {
        return None;
    }
    let height = cur.u64()?;
    let parent = BlockHash(cur.arr32()?);
    let state_root = Commitment(cur.arr32()?);
    let n = cur.u32()? as usize;
    let mut intents = Vec::with_capacity(n);
    for _ in 0..n {
        intents.push(decode_intent(&mut cur)?);
    }
    if !cur.is_eof() {
        return None;
    }
    Some(Block {
        height,
        parent,
        state_root,
        intents,
    })
}

fn encode_intent(intent: &Intent, out: &mut Vec<u8>) {
    match intent {
        Intent::Transfer { from, to, amount } => {
            out.push(0);
            out.extend_from_slice(&from.0);
            out.extend_from_slice(&to.0);
            out.extend_from_slice(&amount.to_be_bytes());
        }
        Intent::Call {
            caller,
            target,
            value,
            calldata,
        } => {
            out.push(1);
            out.extend_from_slice(&caller.0);
            out.extend_from_slice(&target.0);
            out.extend_from_slice(&value.to_be_bytes());
            out.extend_from_slice(&u32::try_from(calldata.len()).unwrap_or(u32::MAX).to_be_bytes());
            out.extend_from_slice(calldata);
        }
    }
}

fn decode_intent(cur: &mut Cursor<'_>) -> Option<Intent> {
    match cur.byte()? {
        0 => Some(Intent::Transfer {
            from: Address(cur.arr20()?),
            to: Address(cur.arr20()?),
            amount: cur.u128()?,
        }),
        1 => {
            let caller = Address(cur.arr20()?);
            let target = Address(cur.arr20()?);
            let value = cur.u128()?;
            let len = cur.u32()? as usize;
            let calldata = cur.bytes(len)?.to_vec();
            Some(Intent::Call {
                caller,
                target,
                value,
                calldata,
            })
        }
        _ => None,
    }
}

struct Cursor<'a> {
    buf: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, at: 0 }
    }
    fn is_eof(&self) -> bool {
        self.at == self.buf.len()
    }
    fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        if end > self.buf.len() {
            return None;
        }
        let s = &self.buf[self.at..end];
        self.at = end;
        Some(s)
    }
    fn byte(&mut self) -> Option<u8> {
        Some(self.bytes(1)?[0])
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.bytes(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_be_bytes(self.bytes(8)?.try_into().ok()?))
    }
    fn u128(&mut self) -> Option<u128> {
        Some(u128::from_be_bytes(self.bytes(16)?.try_into().ok()?))
    }
    fn arr20(&mut self) -> Option<[u8; 20]> {
        self.bytes(20)?.try_into().ok()
    }
    fn arr32(&mut self) -> Option<[u8; 32]> {
        self.bytes(32)?.try_into().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recovery::block::GENESIS_PARENT;
    use crate::Intent;
    use gsxdb_state::{Address, Commitment};
    use tempfile::TempDir;

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


    #[test]
    fn decode_rejects_unknown_version() {
        let mut encoded = vec![9_u8];
        encoded.extend_from_slice(&[0_u8; 8 + 32 + 32 + 4]);
        assert!(decode_block(&encoded).is_none());
    }

    #[test]
    fn decode_rejects_truncated_payload() {
        assert!(decode_block(&[0_u8; 7]).is_none());
    }

    #[test]
    fn redb_store_round_trip_and_restart() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("blocks.redb");

        let mut s = RedbBlockStore::open(&db_path).expect("open redb");
        let b0 = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let b1 = block(1, b0.hash(), Commitment([2; 32]));
        s.put(b0.clone());
        s.put(b1.clone());
        assert_eq!(s.latest(), Some(b1.clone()));
        drop(s);

        let reopened = RedbBlockStore::open(&db_path).expect("reopen redb");
        assert_eq!(reopened.len(), 2);
        assert_eq!(reopened.get_by_height(0), Some(b0));
        assert_eq!(reopened.get_by_height(1), Some(b1));
    }
}
