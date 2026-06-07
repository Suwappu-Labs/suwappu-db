//! L1 anchor registry reader trait and implementations.
//!
//! Reads anchor digests from the Solidity `LTPAnchorRegistry` contract
//! on chain 103115120 (SUWAPPU L1). Supports both mock (in-memory) and
//! production (RPC) backends.

use super::types::{Anchor, ChainId};
use suwappudb_state::Commitment;
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[cfg(test)]
use super::types::GENESIS_PARENT;

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
#[allow(dead_code)]
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

impl RpcL1AnchorReader {
    /// Call `getAnchor(chainId, height)` on LTPAnchorRegistry via eth_call.
    /// Returns the encoded anchor data or an error if the RPC call fails.
    fn call_get_anchor(&self, chain_id: ChainId, height: u64) -> Result<Value, String> {
        // Encode function call: getAnchor(uint32 chainId, uint64 height)
        // Function signature: 0x12345678 (placeholder; real ABI needed)
        // For now, we'll use a simple encoding: 0x + chainId (32 bytes) + height (32 bytes)
        let chain_id_hex = format!("{:064x}", chain_id.0);
        let height_hex = format!("{:064x}", height);
        let data = format!("0x{}{}", chain_id_hex, height_hex);

        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_call",
            "params": [
                {
                    "to": self.registry_addr.clone(),
                    "data": data,
                },
                "latest"
            ],
            "id": 1
        });

        // Make RPC call
        match ureq::post(&self.rpc_url)
            .set("Content-Type", "application/json")
            .send_json(&payload)
        {
            Ok(response) => {
                match response.into_json::<Value>() {
                    Ok(result) => {
                        // Check for JSON-RPC error
                        if let Some(err) = result.get("error") {
                            return Err(format!("RPC error: {}", err));
                        }
                        Ok(result)
                    }
                    Err(e) => Err(format!("Failed to parse response: {}", e)),
                }
            }
            Err(e) => Err(format!("RPC call failed: {}", e)),
        }
    }

    /// Decode eth_call result into an Anchor.
    /// Result should be: (chainId, height, stateRoot, parent, mac)
    fn decode_anchor(
        &self,
        result: &Value,
        chain_id: ChainId,
        height: u64,
        key: &[u8; 32],
    ) -> Option<Anchor> {
        // Extract the 'result' field from JSON-RPC response
        let data_hex = result.get("result")?.as_str()?;

        // Remove '0x' prefix and decode hex
        let data = hex::decode(&data_hex[2..]).ok()?;

        // Expected format (from Solidity contract):
        // 32 bytes: state_root
        // 32 bytes: parent hash
        // 32 bytes: mac
        // (Total: 96 bytes minimum)
        if data.len() < 96 {
            return None;
        }

        let mut state_root_bytes = [0u8; 32];
        state_root_bytes.copy_from_slice(&data[0..32]);

        let mut parent_bytes = [0u8; 32];
        parent_bytes.copy_from_slice(&data[32..64]);

        let mut mac_bytes = [0u8; 32];
        mac_bytes.copy_from_slice(&data[64..96]);

        let state_root = Commitment(state_root_bytes);
        let parent = super::types::AnchorHash(parent_bytes);

        // Verify authenticator before returning
        let anchor = Anchor {
            chain_id,
            height,
            state_root,
            parent,
            mac: mac_bytes,
            auth_scheme: super::types::AuthScheme::Blake3Mac,
        };

        if anchor.verify_auth(key) {
            Some(anchor)
        } else {
            None
        }
    }
}

impl L1AnchorReader for RpcL1AnchorReader {
    fn read_anchor(&self, chain_id: ChainId, height: u64) -> Option<Anchor> {
        // TODO: Get the HMAC key for this chain from dispatcher or config
        // For now, use a placeholder key (should be passed from AnchorDispatcher)
        let placeholder_key = [0u8; 32];

        match self.call_get_anchor(chain_id, height) {
            Ok(result) => self.decode_anchor(&result, chain_id, height, &placeholder_key),
            Err(e) => {
                tracing::warn!("Failed to read anchor from L1 registry: {}", e);
                None
            }
        }
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

    #[test]
    fn rpc_reader_decodes_anchor_response() {
        let reader = RpcL1AnchorReader::new("http://localhost:8545", "0x1234567890abcdef");
        let chain_id = ChainId(1);
        let height = 5u64;
        let key = [0u8; 32];

        // Create an anchor with the key
        let anchor = Anchor::new(chain_id, height, Commitment([42; 32]), GENESIS_PARENT, &key);

        // Build the expected response format
        let mut response_data = Vec::new();
        response_data.extend_from_slice(&anchor.state_root.0); // 32 bytes
        response_data.extend_from_slice(&anchor.parent.0); // 32 bytes
        response_data.extend_from_slice(&anchor.mac); // 32 bytes

        let response_hex = hex::encode(&response_data);
        let result = json!({
            "result": format!("0x{}", response_hex)
        });

        // Decode should succeed
        let decoded = reader.decode_anchor(&result, chain_id, height, &key);
        assert_eq!(decoded, Some(anchor));
    }

    #[test]
    fn rpc_reader_rejects_invalid_mac() {
        let reader = RpcL1AnchorReader::new("http://localhost:8545", "0x1234567890abcdef");
        let chain_id = ChainId(1);
        let height = 5u64;
        let correct_key = [0u8; 32];
        let wrong_key = [1u8; 32];

        // Create anchor with correct key
        let anchor = Anchor::new(
            chain_id,
            height,
            Commitment([42; 32]),
            GENESIS_PARENT,
            &correct_key,
        );

        // Build response
        let mut response_data = Vec::new();
        response_data.extend_from_slice(&anchor.state_root.0);
        response_data.extend_from_slice(&anchor.parent.0);
        response_data.extend_from_slice(&anchor.mac);

        let response_hex = hex::encode(&response_data);
        let result = json!({
            "result": format!("0x{}", response_hex)
        });

        // Decoding with wrong key should return None (MAC verification fails)
        let decoded = reader.decode_anchor(&result, chain_id, height, &wrong_key);
        assert_eq!(decoded, None);
    }
}
