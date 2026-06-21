//! `BlockStore` trait + in-memory impl.
//!
//! Phase-1 ships the in-memory backend; a redb-backed durable store
//! lands in S8.5 alongside production deployment. Same dev/prod
//! pattern as IQ-1 (redb in dev / `RocksDB` in prod for the state
//! store).

use super::block::{Block, BlockHash};
use crate::Intent;
use suwappudb_state::{Address, Commitment};
use redb::{Database, ReadableTable, ReadableTableMetadata, TableDefinition};
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

const BLOCKS_BY_HASH: TableDefinition<[u8; 32], &[u8]> = TableDefinition::new("blocks_by_hash");
const HEIGHT_TO_HASH: TableDefinition<u64, [u8; 32]> = TableDefinition::new("height_to_hash");
const BLOCK_ENCODING_VERSION: u8 = 1;

/// Storage-layer failures surfaced by [`BlockStore`] implementations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockStoreError {
    /// Underlying storage backend error.
    Backend(String),
}

impl BlockStoreError {
    fn backend(err: impl ToString) -> Self {
        Self::Backend(err.to_string())
    }
}

/// Append-only block storage.
pub trait BlockStore {
    /// Append `block`. Stores the block keyed by its hash and indexed
    /// by its height.
    fn put(&mut self, block: Block) -> Result<(), BlockStoreError>;

    /// Lookup by hash.
    fn get_by_hash(&self, hash: &BlockHash) -> Result<Option<Block>, BlockStoreError>;

    /// Lookup by logical height.
    fn get_by_height(&self, height: u64) -> Result<Option<Block>, BlockStoreError>;

    /// Latest block by height, if any.
    fn latest(&self) -> Result<Option<Block>, BlockStoreError>;

    /// Number of blocks.
    fn len(&self) -> Result<usize, BlockStoreError>;

    /// `true` iff no blocks stored.
    fn is_empty(&self) -> bool {
        self.len().map_or(true, |n| n == 0)
    }

    /// Iterate every block in height order, starting at `from`
    /// inclusive. Phase-1 returns a `Vec` for simplicity.
    fn iter_from(&self, from: u64) -> Result<Vec<Block>, BlockStoreError>;
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
    /// **B2**: returns [`BlockStoreError::Backend`] on any redb-side
    /// failure (open / create / begin_write / table init / commit).
    /// Pre-B2 the table-init path called `.expect()` and panicked on
    /// corrupt files; this surfaces the failure as a typed error
    /// callers can decide how to handle.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BlockStoreError> {
        let db = if path.as_ref().exists() {
            Database::open(path).map_err(BlockStoreError::backend)?
        } else {
            Database::create(path).map_err(BlockStoreError::backend)?
        };

        let write_txn = db.begin_write().map_err(BlockStoreError::backend)?;
        {
            write_txn
                .open_table(BLOCKS_BY_HASH)
                .map_err(BlockStoreError::backend)?;
            write_txn
                .open_table(HEIGHT_TO_HASH)
                .map_err(BlockStoreError::backend)?;
        }
        write_txn.commit().map_err(BlockStoreError::backend)?;

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
    fn put(&mut self, block: Block) -> Result<(), BlockStoreError> {
        let hash = block.hash();
        self.by_height.insert(block.height, hash);
        self.by_hash.insert(hash, block);
        Ok(())
    }

    fn get_by_hash(&self, hash: &BlockHash) -> Result<Option<Block>, BlockStoreError> {
        Ok(self.by_hash.get(hash).cloned())
    }

    fn get_by_height(&self, height: u64) -> Result<Option<Block>, BlockStoreError> {
        Ok(self
            .by_height
            .get(&height)
            .and_then(|h| self.by_hash.get(h).cloned()))
    }

    fn latest(&self) -> Result<Option<Block>, BlockStoreError> {
        Ok(self
            .by_height
            .iter()
            .next_back()
            .and_then(|(_, h)| self.by_hash.get(h).cloned()))
    }

    fn len(&self) -> Result<usize, BlockStoreError> {
        Ok(self.by_hash.len())
    }

    fn iter_from(&self, from: u64) -> Result<Vec<Block>, BlockStoreError> {
        Ok(self
            .by_height
            .range(from..)
            .filter_map(|(_, h)| self.by_hash.get(h).cloned())
            .collect())
    }
}

impl BlockStore for RedbBlockStore {
    fn put(&mut self, block: Block) -> Result<(), BlockStoreError> {
        let hash = block.hash();
        let encoded = encode_block(&block);

        let write_txn = self.db.begin_write().map_err(BlockStoreError::backend)?;
        {
            let mut blocks = write_txn
                .open_table(BLOCKS_BY_HASH)
                .map_err(BlockStoreError::backend)?;
            blocks
                .insert(hash.0, encoded.as_slice())
                .map_err(BlockStoreError::backend)?;
        }
        {
            let mut heights = write_txn
                .open_table(HEIGHT_TO_HASH)
                .map_err(BlockStoreError::backend)?;
            heights
                .insert(block.height, hash.0)
                .map_err(BlockStoreError::backend)?;
        }
        write_txn.commit().map_err(BlockStoreError::backend)?;
        Ok(())
    }

    fn get_by_hash(&self, hash: &BlockHash) -> Result<Option<Block>, BlockStoreError> {
        let read_txn = self.db.begin_read().map_err(BlockStoreError::backend)?;
        let table = read_txn
            .open_table(BLOCKS_BY_HASH)
            .map_err(BlockStoreError::backend)?;
        Ok(table
            .get(hash.0)
            .map_err(BlockStoreError::backend)?
            .and_then(|v| decode_block(v.value())))
    }

    fn get_by_height(&self, height: u64) -> Result<Option<Block>, BlockStoreError> {
        let read_txn = self.db.begin_read().map_err(BlockStoreError::backend)?;
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .map_err(BlockStoreError::backend)?;
        let Some(hash) = heights.get(height).map_err(BlockStoreError::backend)? else {
            return Ok(None);
        };
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .map_err(BlockStoreError::backend)?;
        Ok(blocks
            .get(hash.value())
            .map_err(BlockStoreError::backend)?
            .and_then(|v| decode_block(v.value())))
    }

    fn latest(&self) -> Result<Option<Block>, BlockStoreError> {
        let read_txn = self.db.begin_read().map_err(BlockStoreError::backend)?;
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .map_err(BlockStoreError::backend)?;
        let (_, hash) = heights
            .last()
            .map_err(BlockStoreError::backend)?
            .ok_or_else(|| BlockStoreError::backend("missing latest height entry"))?;
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .map_err(BlockStoreError::backend)?;
        Ok(blocks
            .get(hash.value())
            .map_err(BlockStoreError::backend)?
            .and_then(|v| decode_block(v.value())))
    }

    fn len(&self) -> Result<usize, BlockStoreError> {
        let read_txn = self.db.begin_read().map_err(BlockStoreError::backend)?;
        let table = read_txn
            .open_table(BLOCKS_BY_HASH)
            .map_err(BlockStoreError::backend)?;
        Ok(table
            .len()
            .map_err(BlockStoreError::backend)?
            .try_into()
            .unwrap_or(usize::MAX))
    }

    fn iter_from(&self, from: u64) -> Result<Vec<Block>, BlockStoreError> {
        let read_txn = self.db.begin_read().map_err(BlockStoreError::backend)?;
        let heights = read_txn
            .open_table(HEIGHT_TO_HASH)
            .map_err(BlockStoreError::backend)?;
        let blocks = read_txn
            .open_table(BLOCKS_BY_HASH)
            .map_err(BlockStoreError::backend)?;

        let mut out = Vec::new();
        for row in heights.range(from..).map_err(BlockStoreError::backend)? {
            let (_, h) = row.map_err(BlockStoreError::backend)?;
            if let Some(v) = blocks
                .get(h.value())
                .map_err(BlockStoreError::backend)?
                .and_then(|v| decode_block(v.value()))
            {
                out.push(v);
            }
        }
        Ok(out)
    }
}

fn encode_block(block: &Block) -> Vec<u8> {
    let mut out = Vec::new();
    out.push(BLOCK_ENCODING_VERSION);
    out.extend_from_slice(&block.height.to_be_bytes());
    out.extend_from_slice(&block.parent.0);
    out.extend_from_slice(&block.state_root.0);
    out.extend_from_slice(
        &u32::try_from(block.intents.len())
            .unwrap_or(u32::MAX)
            .to_be_bytes(),
    );
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
            out.extend_from_slice(
                &u32::try_from(calldata.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            out.extend_from_slice(calldata);
        }
        Intent::DeployModule {
            account,
            name,
            bytes,
        } => {
            // tag 2: DeployModule (S9.3)
            out.push(2);
            out.extend_from_slice(&account.0);
            let name_bytes = name.as_str().as_bytes();
            out.extend_from_slice(
                &u32::try_from(name_bytes.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            out.extend_from_slice(name_bytes);
            out.extend_from_slice(
                &u32::try_from(bytes.len())
                    .unwrap_or(u32::MAX)
                    .to_be_bytes(),
            );
            out.extend_from_slice(bytes);
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
        2 => {
            // tag 2: DeployModule (S9.3)
            let account = suwappudb_state::MoveAddress(cur.arr32()?);
            let name_len = cur.u32()? as usize;
            let name_bytes = cur.bytes(name_len)?.to_vec();
            let name_str = std::str::from_utf8(&name_bytes).ok()?;
            let name = suwappudb_state::Identifier::new(name_str).ok()?;
            let bytes_len = cur.u32()? as usize;
            let bytes = cur.bytes(bytes_len)?.to_vec();
            Some(Intent::DeployModule {
                account,
                name,
                bytes,
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
    use suwappudb_state::{Address, Commitment};
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
        assert_eq!(s.len(), Ok(0));
        assert_eq!(s.latest(), Ok(None));
        assert_eq!(s.get_by_height(0), Ok(None));
    }

    #[test]
    fn put_and_lookup_round_trip() {
        let mut s = InMemoryBlockStore::new();
        let b = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let hash = b.hash();
        s.put(b.clone()).unwrap();

        assert_eq!(s.len(), Ok(1));
        assert_eq!(s.get_by_hash(&hash), Ok(Some(b.clone())));
        assert_eq!(s.get_by_height(0), Ok(Some(b.clone())));
        assert_eq!(s.latest(), Ok(Some(b)));
    }

    #[test]
    fn latest_tracks_highest_height() {
        let mut s = InMemoryBlockStore::new();
        let b0 = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let b1 = block(1, b0.hash(), Commitment([2; 32]));
        let b2 = block(2, b1.hash(), Commitment([3; 32]));
        s.put(b1.clone()).unwrap(); // out of order
        s.put(b2.clone()).unwrap();
        s.put(b0.clone()).unwrap();

        assert_eq!(s.latest(), Ok(Some(b2)));
        assert_eq!(s.get_by_height(0), Ok(Some(b0)));
        assert_eq!(s.get_by_height(1), Ok(Some(b1)));
    }

    #[test]
    fn iter_from_filters_by_height() {
        let mut s = InMemoryBlockStore::new();
        let b0 = block(0, GENESIS_PARENT, Commitment([1; 32]));
        let b1 = block(1, b0.hash(), Commitment([2; 32]));
        let b2 = block(2, b1.hash(), Commitment([3; 32]));
        s.put(b0).unwrap();
        s.put(b1.clone()).unwrap();
        s.put(b2.clone()).unwrap();

        let from_1 = s.iter_from(1).unwrap();
        assert_eq!(from_1.len(), 2);
        assert_eq!(from_1[0], b1);
        assert_eq!(from_1[1], b2);

        let from_5 = s.iter_from(5).unwrap();
        assert!(from_5.is_empty());
    }

    #[test]
    fn deploy_module_intent_encode_decode_round_trip() {
        // S9.3: DeployModule round-trips through the block-store
        // encoded format (tag 2 + 32B account + name + bytes).
        let account = suwappudb_state::MoveAddress([7; 32]);
        let name = suwappudb_state::Identifier::new("payments").unwrap();
        let bytes = vec![0xCA, 0xFE, 0xBA, 0xBE, 0x12, 0x34];
        let intent = Intent::DeployModule {
            account,
            name: name.clone(),
            bytes: bytes.clone(),
        };

        // Encode into a block; round-trip through the persistent store.
        let blk = Block {
            height: 0,
            parent: GENESIS_PARENT,
            state_root: Commitment([0; 32]),
            intents: vec![intent.clone()],
        };
        let tmp = TempDir::new().expect("tempdir");
        let path = tmp.path().join("blocks.redb");
        let mut s = RedbBlockStore::open(&path).expect("open");
        s.put(blk.clone()).expect("put");
        let recovered = s
            .get_by_height(0)
            .expect("get")
            .expect("present")
            .intents
            .into_iter()
            .next()
            .expect("intent present");
        assert_eq!(recovered, intent);
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
        s.put(b0.clone()).unwrap();
        s.put(b1.clone()).unwrap();
        assert_eq!(s.latest(), Ok(Some(b1.clone())));
        drop(s);

        let reopened = RedbBlockStore::open(&db_path).expect("reopen redb");
        assert_eq!(reopened.len(), Ok(2));
        assert_eq!(reopened.get_by_height(0), Ok(Some(b0)));
        assert_eq!(reopened.get_by_height(1), Ok(Some(b1)));
    }

    #[test]
    fn redb_recover_across_restart_with_many_blocks() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("blocks.redb");

        // Phase 1: write 10 blocks
        let mut s = RedbBlockStore::open(&db_path).expect("open redb");
        let mut blocks = vec![];
        let mut parent = GENESIS_PARENT;
        for i in 0..10u64 {
            let b = Block {
                height: i,
                parent,
                state_root: Commitment([i as u8; 32]),
                intents: vec![Intent::Transfer {
                    from: addr(0),
                    to: addr(1),
                    amount: u128::from(i),
                }],
            };
            parent = b.hash();
            s.put(b.clone()).unwrap();
            blocks.push(b);
        }
        drop(s);

        // Phase 2: reopen and verify all blocks recovered.
        let reopened = RedbBlockStore::open(&db_path).expect("reopen redb");
        assert_eq!(reopened.len(), Ok(10));

        for (i, expected) in blocks.iter().enumerate() {
            let retrieved = reopened.get_by_height(i as u64).unwrap();
            assert_eq!(
                retrieved,
                Some(expected.clone()),
                "block {i} mismatch after restart"
            );
        }

        let all = reopened.iter_from(0).unwrap();
        assert_eq!(all.len(), 10);
        for (i, expected) in blocks.iter().enumerate() {
            assert_eq!(all[i], *expected);
        }

        let from_5 = reopened.iter_from(5).unwrap();
        assert_eq!(from_5.len(), 5);
        for (i, expected) in blocks[5..].iter().enumerate() {
            assert_eq!(from_5[i], *expected);
        }
    }

    #[test]
    fn redb_multiple_restarts_preserve_state() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("blocks.redb");

        // First cycle: write blocks 0-3
        {
            let mut s = RedbBlockStore::open(&db_path).expect("open redb cycle 1");
            let mut parent = GENESIS_PARENT;
            for i in 0..4u64 {
                let b = Block {
                    height: i,
                    parent,
                    state_root: Commitment([i as u8; 32]),
                    intents: vec![],
                };
                parent = b.hash();
                s.put(b).unwrap();
            }
        }

        // Second cycle: reopen, add blocks 4-6
        {
            let mut s = RedbBlockStore::open(&db_path).expect("open redb cycle 2");
            assert_eq!(s.len(), Ok(4));
            let mut parent = s.latest().unwrap().expect("latest").hash();
            for i in 4..7u64 {
                let b = Block {
                    height: i,
                    parent,
                    state_root: Commitment([i as u8; 32]),
                    intents: vec![],
                };
                parent = b.hash();
                s.put(b).unwrap();
            }
        }

        // Third cycle: verify all blocks are still there.
        {
            let s = RedbBlockStore::open(&db_path).expect("open redb cycle 3");
            assert_eq!(s.len(), Ok(7));
            assert_eq!(s.get_by_height(0).unwrap().map(|b| b.height), Some(0));
            assert_eq!(s.get_by_height(6).unwrap().map(|b| b.height), Some(6));
        }
    }

    #[test]
    fn redb_corrupt_payload_is_rejected_without_panic() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("blocks.redb");
        let db = Database::create(&db_path).expect("create redb");

        let bad_hash = [0xabu8; 32];
        let write_txn = db.begin_write().expect("begin_write");
        {
            let mut blocks = write_txn
                .open_table(BLOCKS_BY_HASH)
                .expect("open blocks table");
            blocks
                .insert(bad_hash, [BLOCK_ENCODING_VERSION, 1, 2, 3].as_slice())
                .expect("insert bad payload");
            let mut heights = write_txn
                .open_table(HEIGHT_TO_HASH)
                .expect("open heights table");
            heights
                .insert(7, bad_hash)
                .expect("insert bad height->hash");
        }
        write_txn.commit().expect("commit bad payload");
        drop(db);

        let reopened = RedbBlockStore::open(&db_path).expect("reopen redb");
        assert_eq!(reopened.get_by_hash(&BlockHash(bad_hash)), Ok(None));
        assert_eq!(reopened.get_by_height(7), Ok(None));
        assert_eq!(reopened.iter_from(0).unwrap().len(), 0);
    }

    #[test]
    fn redb_aborted_write_txn_leaves_no_partial_state() {
        let tmp = TempDir::new().expect("tempdir");
        let db_path = tmp.path().join("blocks.redb");
        let db = Database::create(&db_path).expect("create redb");

        let fake_hash = [0x42_u8; 32];
        let write_txn = db.begin_write().expect("begin_write");
        {
            let mut blocks = write_txn
                .open_table(BLOCKS_BY_HASH)
                .expect("open blocks table");
            blocks
                .insert(fake_hash, [BLOCK_ENCODING_VERSION, 0, 0, 0].as_slice())
                .expect("insert uncommitted payload");
        }
        drop(write_txn);
        drop(db);

        let reopened = RedbBlockStore::open(&db_path).expect("reopen redb");
        assert_eq!(reopened.get_by_hash(&BlockHash(fake_hash)), Ok(None));
        assert_eq!(reopened.get_by_height(0), Ok(None));
        assert_eq!(reopened.len(), Ok(0));
        assert!(reopened.iter_from(0).unwrap().is_empty());
    }
}
