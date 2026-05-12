//! DAG (Directed Acyclic Graph) store for flexible block dependencies.
//!
//! Instead of linear chain (block N → block N-1), supports block N → any earlier block.
//! Enables skipping redundant blocks that commit the same state.

use std::collections::{BTreeMap, VecDeque};

/// Block hash (32 bytes).
pub type BlockHash = [u8; 32];

/// A block in the DAG with flexible parent linkage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagBlock {
    /// Block height in the canonical chain.
    pub height: u64,
    /// State commitment after executing this block.
    pub state_root: [u8; 32],
    /// Parent block hash(es). Can point to any earlier block, not just height-1.
    /// For linear chain: vec![[previous_hash]]
    /// For DAG: may contain multiple parents (conditional branches).
    pub parent_hashes: Vec<BlockHash>,
    /// Block timestamp (seconds since epoch).
    pub timestamp: u64,
}

impl DagBlock {
    /// Create a new block with a single parent (linear chain compatible).
    pub fn new(height: u64, state_root: [u8; 32], parent_hash: BlockHash, timestamp: u64) -> Self {
        Self {
            height,
            state_root,
            parent_hashes: vec![parent_hash],
            timestamp,
        }
    }

    /// Create genesis block (no parents).
    pub fn genesis(state_root: [u8; 32], timestamp: u64) -> Self {
        Self {
            height: 0,
            state_root,
            parent_hashes: Vec::new(),
            timestamp,
        }
    }
}

/// In-memory DAG store. Maps block hash → DagBlock.
#[derive(Debug, Clone)]
pub struct DagStore {
    blocks: BTreeMap<BlockHash, DagBlock>,
}

impl DagStore {
    /// Create an empty DAG.
    pub fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
        }
    }

    /// Insert a block into the DAG.
    pub fn put(&mut self, hash: BlockHash, block: DagBlock) {
        self.blocks.insert(hash, block);
    }

    /// Retrieve a block by hash.
    pub fn get(&self, hash: &BlockHash) -> Option<&DagBlock> {
        self.blocks.get(hash)
    }

    /// Check if a block exists.
    pub fn contains(&self, hash: &BlockHash) -> bool {
        self.blocks.contains_key(hash)
    }

    /// Number of blocks in the DAG.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Check if DAG is empty.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Find a path from `from` to `to` using BFS.
    /// Returns `Some(path)` if reachable, `None` otherwise.
    pub fn find_path(&self, from: &BlockHash, to: &BlockHash) -> Option<Vec<BlockHash>> {
        if from == to {
            return Some(vec![*from]);
        }

        let mut queue = VecDeque::new();
        let mut visited = std::collections::HashSet::new();
        let mut parent_map = std::collections::HashMap::new();

        queue.push_back(*from);
        visited.insert(*from);

        while let Some(current) = queue.pop_front() {
            if let Some(block) = self.get(&current) {
                for &parent in &block.parent_hashes {
                    if parent == *to {
                        // Found target; reconstruct path
                        let mut path = vec![parent];
                        let mut node = current;
                        while let Some(&prev) = parent_map.get(&node) {
                            path.push(node);
                            node = prev;
                        }
                        path.push(*from);
                        path.reverse();
                        return Some(path);
                    }

                    if !visited.contains(&parent) {
                        visited.insert(parent);
                        parent_map.insert(parent, current);
                        queue.push_back(parent);
                    }
                }
            }
        }

        None
    }

    /// Check if block `to` is reachable from block `from`.
    pub fn is_reachable(&self, from: &BlockHash, to: &BlockHash) -> bool {
        self.find_path(from, to).is_some()
    }

    /// Get all blocks at a given height.
    pub fn blocks_at_height(&self, height: u64) -> Vec<(BlockHash, &DagBlock)> {
        self.blocks
            .iter()
            .filter(|(_, b)| b.height == height)
            .map(|(h, b)| (*h, b))
            .collect()
    }

    /// Get the block with maximum height.
    pub fn max_height_block(&self) -> Option<(BlockHash, &DagBlock)> {
        self.blocks
            .iter()
            .max_by_key(|(_, b)| b.height)
            .map(|(h, b)| (*h, b))
    }

    /// Validate the DAG structure:
    /// - All parents must exist
    /// - Heights must be strictly increasing along any path
    /// - No cycles
    pub fn validate(&self) -> Result<(), String> {
        // Check all parents exist
        for (hash, block) in &self.blocks {
            for &parent_hash in &block.parent_hashes {
                if !self.blocks.contains_key(&parent_hash) {
                    return Err(format!(
                        "Block {:?} references non-existent parent {:?}",
                        hex::encode(&hash[..4]),
                        hex::encode(&parent_hash[..4])
                    ));
                }

                let parent = &self.blocks[&parent_hash];
                if parent.height >= block.height {
                    return Err(format!(
                        "Block height {} must be > parent height {}",
                        block.height, parent.height
                    ));
                }
            }
        }

        // Check for cycles using DFS from each node
        for (start_hash, _) in &self.blocks {
            if self.has_cycle_from(start_hash) {
                return Err(format!(
                    "Cycle detected from block {:?}",
                    hex::encode(&start_hash[..4])
                ));
            }
        }

        Ok(())
    }

    /// Helper: check if there's a cycle starting from a given block.
    fn has_cycle_from(&self, start: &BlockHash) -> bool {
        let mut visited = std::collections::HashSet::new();
        let mut rec_stack = std::collections::HashSet::new();
        self.dfs_cycle(start, &mut visited, &mut rec_stack)
    }

    /// DFS helper for cycle detection.
    fn dfs_cycle(
        &self,
        node: &BlockHash,
        visited: &mut std::collections::HashSet<BlockHash>,
        rec_stack: &mut std::collections::HashSet<BlockHash>,
    ) -> bool {
        visited.insert(*node);
        rec_stack.insert(*node);

        if let Some(block) = self.get(node) {
            for &parent in &block.parent_hashes {
                if !visited.contains(&parent) {
                    if self.dfs_cycle(&parent, visited, rec_stack) {
                        return true;
                    }
                } else if rec_stack.contains(&parent) {
                    return true;
                }
            }
        }

        rec_stack.remove(node);
        false
    }
}

impl Default for DagStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(byte: u8) -> BlockHash {
        [byte; 32]
    }

    #[test]
    fn dag_empty_is_empty() {
        let dag = DagStore::new();
        assert!(dag.is_empty());
        assert_eq!(dag.len(), 0);
    }

    #[test]
    fn dag_put_and_get_round_trip() {
        let mut dag = DagStore::new();
        let block = DagBlock::genesis([1u8; 32], 1000);
        dag.put(hash(1), block.clone());

        assert_eq!(dag.get(&hash(1)), Some(&block));
        assert_eq!(dag.len(), 1);
    }

    #[test]
    fn dag_linear_chain() {
        let mut dag = DagStore::new();

        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b2 = DagBlock::new(2, [2u8; 32], hash(1), 3000);

        dag.put(hash(0), b0);
        dag.put(hash(1), b1);
        dag.put(hash(2), b2);

        assert!(dag.is_reachable(&hash(0), &hash(0)));
        assert!(dag.is_reachable(&hash(2), &hash(0)));
        assert!(!dag.is_reachable(&hash(0), &hash(2)));
    }

    #[test]
    fn dag_branching() {
        let mut dag = DagStore::new();

        // Genesis
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        dag.put(hash(0), b0);

        // Two blocks at height 1, both pointing to genesis
        let b1a = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b1b = DagBlock::new(1, [10u8; 32], hash(0), 2100);
        dag.put(hash(1), b1a);
        dag.put(hash(10), b1b);

        // Both are reachable from genesis
        assert!(dag.is_reachable(&hash(1), &hash(0)));
        assert!(dag.is_reachable(&hash(10), &hash(0)));
    }

    #[test]
    fn dag_find_path() {
        let mut dag = DagStore::new();

        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b2 = DagBlock::new(2, [2u8; 32], hash(1), 3000);

        dag.put(hash(0), b0);
        dag.put(hash(1), b1);
        dag.put(hash(2), b2);

        let path = dag.find_path(&hash(2), &hash(0)).unwrap();
        assert_eq!(path, vec![hash(2), hash(1), hash(0)]);
    }

    #[test]
    fn dag_validation_detects_missing_parent() {
        let mut dag = DagStore::new();
        let b1 = DagBlock::new(1, [1u8; 32], hash(99), 2000);
        dag.put(hash(1), b1);

        assert!(dag.validate().is_err());
    }

    #[test]
    fn dag_validation_detects_height_violation() {
        let mut dag = DagStore::new();

        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b0 = DagBlock::new(0, [0u8; 32], hash(1), 1000); // Parent is higher!

        dag.put(hash(1), b1);
        dag.put(hash(0), b0);

        assert!(dag.validate().is_err());
    }

    #[test]
    fn dag_blocks_at_height() {
        let mut dag = DagStore::new();

        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1a = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b1b = DagBlock::new(1, [11u8; 32], hash(0), 2100);

        dag.put(hash(0), b0);
        dag.put(hash(1), b1a);
        dag.put(hash(11), b1b);

        assert_eq!(dag.blocks_at_height(0).len(), 1);
        assert_eq!(dag.blocks_at_height(1).len(), 2);
        assert_eq!(dag.blocks_at_height(2).len(), 0);
    }

    #[test]
    fn dag_max_height() {
        let mut dag = DagStore::new();

        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b10 = DagBlock::new(10, [10u8; 32], hash(0), 11000);

        dag.put(hash(0), b0);
        dag.put(hash(1), b1);
        dag.put(hash(10), b10);

        let (max_hash, max_block) = dag.max_height_block().unwrap();
        assert_eq!(max_block.height, 10);
        assert_eq!(max_hash, hash(10));
    }
}
