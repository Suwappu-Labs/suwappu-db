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

/// In-memory DAG store. Maps block hash → [`DagBlock`].
///
/// **S12.1**: maintains a children index alongside the primary map so
/// downward traversal (`descendants_of`, `children_of`) is O(deg) per
/// step instead of O(N). The two maps are kept in sync inside
/// [`Self::put`].
#[derive(Debug, Clone)]
pub struct DagStore {
    blocks: BTreeMap<BlockHash, DagBlock>,
    /// `parent_hash` → list of blocks whose `parent_hashes` contains it.
    /// Populated incrementally by [`Self::put`]. Children may appear
    /// before parents (out-of-order ingest), in which case the
    /// children entry exists before the parent's `blocks` entry does.
    children: BTreeMap<BlockHash, Vec<BlockHash>>,
}

impl DagStore {
    /// Create an empty DAG.
    pub fn new() -> Self {
        Self {
            blocks: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }

    /// Insert a block into the DAG. Updates the children index by
    /// appending this block's hash under each of its parents.
    ///
    /// **Idempotency**: re-inserting the same hash overwrites the
    /// block but does NOT duplicate child entries — we check for
    /// existing membership in each parent's child list.
    pub fn put(&mut self, hash: BlockHash, block: DagBlock) {
        for parent in &block.parent_hashes {
            let entry = self.children.entry(*parent).or_default();
            if !entry.contains(&hash) {
                entry.push(hash);
            }
        }
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

    /// **S12.1** — Direct children of `parent` (one level down).
    /// O(deg) lookup via the children index. Returns an empty slice
    /// if `parent` is a leaf or unknown.
    pub fn children_of(&self, parent: &BlockHash) -> &[BlockHash] {
        self.children
            .get(parent)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// **S12.1** — All ancestors of `start` (transitively, up to
    /// genesis). Result is in BFS order from `start`'s parents
    /// outward. `start` itself is NOT included.
    pub fn ancestors_of(&self, start: &BlockHash) -> Vec<BlockHash> {
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: VecDeque<BlockHash> = VecDeque::new();
        if let Some(block) = self.blocks.get(start) {
            for p in &block.parent_hashes {
                if visited.insert(*p) {
                    queue.push_back(*p);
                }
            }
        }
        while let Some(current) = queue.pop_front() {
            out.push(current);
            if let Some(block) = self.blocks.get(&current) {
                for p in &block.parent_hashes {
                    if visited.insert(*p) {
                        queue.push_back(*p);
                    }
                }
            }
        }
        out
    }

    /// **S12.1** — All descendants of `start` (transitively, down to
    /// leaves). Result is in BFS order from `start`'s children
    /// outward. `start` itself is NOT included.
    pub fn descendants_of(&self, start: &BlockHash) -> Vec<BlockHash> {
        let mut out = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue: VecDeque<BlockHash> = VecDeque::new();
        for c in self.children_of(start) {
            if visited.insert(*c) {
                queue.push_back(*c);
            }
        }
        while let Some(current) = queue.pop_front() {
            out.push(current);
            for c in self.children_of(&current) {
                if visited.insert(*c) {
                    queue.push_back(*c);
                }
            }
        }
        out
    }

    /// **S12.1** — Tip hashes: blocks with no children. Useful for
    /// reorg detection ("which heads are live?") and for snapshot
    /// pruning (anything not in any tip's ancestor closure is
    /// reorged out).
    pub fn tips(&self) -> Vec<BlockHash> {
        self.blocks
            .keys()
            .filter(|h| self.children_of(h).is_empty())
            .copied()
            .collect()
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
    fn dag_children_index_tracks_inserts() {
        // S12.1: put a parent and two children; both children appear
        // under children_of(parent).
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1a = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b1b = DagBlock::new(1, [11u8; 32], hash(0), 2100);
        dag.put(hash(0), b0);
        dag.put(hash(1), b1a);
        dag.put(hash(11), b1b);

        let kids = dag.children_of(&hash(0));
        assert_eq!(kids.len(), 2);
        assert!(kids.contains(&hash(1)));
        assert!(kids.contains(&hash(11)));
    }

    #[test]
    fn dag_children_index_is_idempotent_on_reput() {
        // S12.1: re-putting the same hash doesn't duplicate the
        // child entry under its parent.
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        dag.put(hash(0), b0);
        dag.put(hash(1), b1.clone());
        dag.put(hash(1), b1);
        assert_eq!(dag.children_of(&hash(0)), &[hash(1)]);
    }

    #[test]
    fn dag_ancestors_returns_transitive_closure() {
        // S12.1: ancestors_of walks all the way to genesis.
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b2 = DagBlock::new(2, [2u8; 32], hash(1), 3000);
        let b3 = DagBlock::new(3, [3u8; 32], hash(2), 4000);
        dag.put(hash(0), b0);
        dag.put(hash(1), b1);
        dag.put(hash(2), b2);
        dag.put(hash(3), b3);

        let ancs = dag.ancestors_of(&hash(3));
        assert_eq!(ancs, vec![hash(2), hash(1), hash(0)]);
    }

    #[test]
    fn dag_descendants_returns_transitive_closure() {
        // S12.1: descendants_of walks all the way to leaves.
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1 = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b2 = DagBlock::new(2, [2u8; 32], hash(1), 3000);
        dag.put(hash(0), b0);
        dag.put(hash(1), b1);
        dag.put(hash(2), b2);

        let descs = dag.descendants_of(&hash(0));
        assert_eq!(descs, vec![hash(1), hash(2)]);
    }

    #[test]
    fn dag_tips_finds_leaves() {
        // S12.1: tips() returns leaf blocks. With one fork, both
        // forks appear.
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let b1a = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let b1b = DagBlock::new(1, [11u8; 32], hash(0), 2100);
        dag.put(hash(0), b0);
        dag.put(hash(1), b1a);
        dag.put(hash(11), b1b);

        let tips = dag.tips();
        assert_eq!(tips.len(), 2);
        assert!(tips.contains(&hash(1)));
        assert!(tips.contains(&hash(11)));
        // Genesis has children, so it's NOT a tip.
        assert!(!tips.contains(&hash(0)));
    }

    #[test]
    fn dag_multi_parent_descendants_traversed_once() {
        // S12.1: a diamond DAG (G → A,B → C) should yield C once,
        // not twice, in descendants_of(G).
        let mut dag = DagStore::new();
        let b0 = DagBlock::genesis([0u8; 32], 1000);
        let ba = DagBlock::new(1, [1u8; 32], hash(0), 2000);
        let bb = DagBlock::new(1, [2u8; 32], hash(0), 2100);
        let mut bc = DagBlock::new(2, [3u8; 32], hash(1), 3000);
        bc.parent_hashes = vec![hash(1), hash(2)];
        dag.put(hash(0), b0);
        dag.put(hash(1), ba);
        dag.put(hash(2), bb);
        dag.put(hash(3), bc);

        let descs = dag.descendants_of(&hash(0));
        let count_3 = descs.iter().filter(|h| **h == hash(3)).count();
        assert_eq!(count_3, 1, "diamond descendant visited twice");
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
