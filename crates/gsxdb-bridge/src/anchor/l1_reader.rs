//! L1 anchor registry reader trait and implementations.
//!
//! Reads anchor digests from the Solidity `LTPAnchorRegistry` contract
//! on chain 103115120 (GSX L1). Supports both mock (in-memory) and
//! production (RPC) backends.

use super::types::{Anchor, ChainId, GENESIS_PARENT};
use gsxdb_state::Commitment;
use std::collections::BTreeMap;

/// Trait for reading anchors from L1 registry.
pub trait L1AnchorReader: Send + Sync {
    /// Read anchor at `height` for `chain_id`. Returns `None` if not found.
    fn read_anchor(&self, chain_id: ChainId, height: u64) -> Option<Anchor>;
}

/// Mock L1 anchor reader for testing. Stores anchors in memory.
#[derive(Debug, Clone)]
pub struct MockL1AnchorReader {
    /// Map from (chain_id, height) to anchor
    anchors: BTreeMap<(ChainId, u64), Anchor>,
}

impl MockL1AnchorReader {
    /// Create a new empty mock reader.
    #[must_use]
    pub fn new() -> Self {
        Self {
            anchors: BTreeMap::new(),
        }
    }

    /// Insert an anchor for testing.
    pub fn insert(&mut self, chain_id: ChainId, height: u64, anchor: Anchor) {
        self.anchors.insert((chain_id, height), anchor);
    }
}

impl Default for MockL1AnchorReader {
    fn default() -> Self {
        Self::new()
    }
}

impl L1AnchorReader for MockL1AnchorReader {
    fn read_anchor(&self, chain_id: ChainId, height: u64) -> Option<Anchor> {
        self.anchors.get(&(chain_id, height)).cloned()
    }
}

/// Production L1 anchor reader that calls op-reth via JSON-RPC.
/// Calls `eth_call` on `LTPAnchorRegistry` contract at address `registry_addr`.
#[derive(Debug, Clone)]
pub struct RpcL1AnchorReader {
    /// URL of the op-reth JSON-RPC endpoint (e.g., http://localhost:8545)
    rpc_url: String,
    /// Contract address of LTPAnchorRegistry
    registry_addr: String,
}

impl RpcL1AnchorReader {
    /// Create a new RPC reader.
    ///
    /// # Arguments
    ///
    /// * `rpc_url` - JSON-RPC endpoint URL (e.g., http://localhost:8545)
    /// * `registry_addr` - Solidity contract address (0x-prefixed hex)
    pub fn new(rpc_url: impl Into<String>, registry_addr: impl Into<String>) -> Self {
        Self {
            rpc_url: rpc_url.into(),
            registry_addr: registry_addr.into(),
        }
    }
}

impl L1AnchorReader for RpcL1AnchorReader {
    fn read_anchor(&self, _chain_id: ChainId, _height: u64) -> Option<Anchor> {
        // TODO: Implement eth_call to LTPAnchorRegistry
        // For phase-1, this is a placeholder.
        // Phase-1 uses in-memory logs; real L1 registry is launch-readiness (IQ-7).
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_reader_stores_and_retrieves() {
        let mut reader = MockL1AnchorReader::new();
        let chain_id = ChainId(1);
        let anchor = Anchor::new(chain_id, 5, Commitment([42; 32]), GENESIS_PARENT, &[0; 32]);
        reader.insert(chain_id, 5, anchor.clone());

        assert_eq!(reader.read_anchor(chain_id, 5), Some(anchor));
        assert_eq!(reader.read_anchor(chain_id, 6), None);
        assert_eq!(reader.read_anchor(ChainId(2), 5), None);
    }
}
