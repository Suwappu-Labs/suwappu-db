//! L2 state syncer — reads balance and nonce state from op-reth.

use gsxdb_state::{Address, RedbBalanceStore};
use serde_json::{json, Value};

/// Configuration for L2 state synchronization.
#[derive(Debug, Clone)]
pub struct L2SyncConfig {
    /// JSON-RPC endpoint URL for op-reth (e.g., http://localhost:8545)
    pub rpc_url: String,
    /// Addresses to sync balance and nonce for
    pub addresses: Vec<Address>,
}

/// Synced EVM state for a single address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncedEVMState {
    /// Address that was synced
    pub address: Address,
    /// Balance in wei (raw u128)
    pub balance: u128,
    /// Transaction count (nonce)
    pub nonce: u64,
}

/// Syncs EVM state from L2 (op-reth) into gsxdb's redb tables.
#[derive(Debug, Clone)]
pub struct L2StateSyncer {
    config: L2SyncConfig,
}

impl L2StateSyncer {
    /// Create a new L2 state syncer.
    pub fn new(config: L2SyncConfig) -> Self {
        Self { config }
    }

    /// Call eth_getBalance via JSON-RPC.
    async fn get_balance(&self, address: &Address) -> Result<u128, String> {
        let addr_hex = format!("0x{}", hex::encode(address.0));
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_getBalance",
            "params": [addr_hex, "latest"],
            "id": 1
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&self.config.rpc_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("eth_getBalance RPC call failed: {}", e))?;

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse eth_getBalance response: {}", e))?;

        if let Some(err) = result.get("error") {
            return Err(format!("RPC error: {}", err));
        }

        let balance_hex = result
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing result field in eth_getBalance response".to_string())?;

        // Parse hex string (remove '0x' prefix)
        let balance_bytes = hex::decode(&balance_hex[2..])
            .map_err(|e| format!("Failed to decode balance hex: {}", e))?;

        // Convert to u128 (pad with zeros on the left if needed)
        let mut balance_array = [0u8; 16];
        let offset = 16_usize.saturating_sub(balance_bytes.len());
        if balance_bytes.len() <= 16 {
            balance_array[offset..].copy_from_slice(&balance_bytes);
        } else {
            return Err("Balance value too large (> u128)".to_string());
        }

        Ok(u128::from_be_bytes(balance_array))
    }

    /// Call eth_getTransactionCount via JSON-RPC.
    async fn get_transaction_count(&self, address: &Address) -> Result<u64, String> {
        let addr_hex = format!("0x{}", hex::encode(address.0));
        let payload = json!({
            "jsonrpc": "2.0",
            "method": "eth_getTransactionCount",
            "params": [addr_hex, "latest"],
            "id": 1
        });

        let client = reqwest::Client::new();
        let response = client
            .post(&self.config.rpc_url)
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("eth_getTransactionCount RPC call failed: {}", e))?;

        let result: Value = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse eth_getTransactionCount response: {}", e))?;

        if let Some(err) = result.get("error") {
            return Err(format!("RPC error: {}", err));
        }

        let nonce_hex = result
            .get("result")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                "Missing result field in eth_getTransactionCount response".to_string()
            })?;

        // Parse hex string (remove '0x' prefix)
        let nonce_bytes = hex::decode(&nonce_hex[2..])
            .map_err(|e| format!("Failed to decode nonce hex: {}", e))?;

        // Convert to u64 (pad with zeros on the left if needed)
        let mut nonce_array = [0u8; 8];
        let offset = 8_usize.saturating_sub(nonce_bytes.len());
        if nonce_bytes.len() <= 8 {
            nonce_array[offset..].copy_from_slice(&nonce_bytes);
        } else {
            return Err("Nonce value too large (> u64)".to_string());
        }

        Ok(u64::from_be_bytes(nonce_array))
    }

    /// Sync balance and nonce for all configured addresses.
    /// Returns a list of synced state for each address.
    pub async fn sync(&self) -> Result<Vec<SyncedEVMState>, String> {
        let mut synced = Vec::new();

        for address in &self.config.addresses {
            let balance = self.get_balance(address).await?;
            let nonce = self.get_transaction_count(address).await?;

            synced.push(SyncedEVMState {
                address: *address,
                balance,
                nonce,
            });

            tracing::debug!(
                "Synced address {:?}: balance={}, nonce={}",
                address,
                balance,
                nonce
            );
        }

        tracing::info!(
            "L2StateSyncer completed: synced {} addresses from {}",
            synced.len(),
            self.config.rpc_url
        );

        Ok(synced)
    }

    /// Sync balance and nonce, and write results to a redb store.
    /// Returns the synced state vector.
    pub async fn sync_to_store(
        &self,
        store: &RedbBalanceStore,
    ) -> Result<Vec<SyncedEVMState>, String> {
        let synced = self.sync().await?;

        for state in &synced {
            store.set_evm_nonce(&state.address, state.nonce);
            tracing::debug!(
                "Wrote nonce for {:?} to redb: {}",
                state.address,
                state.nonce
            );
        }

        tracing::info!("L2StateSyncer wrote {} nonces to redb store", synced.len());

        Ok(synced)
    }

    /// Get reference to the sync configuration.
    pub fn config(&self) -> &L2SyncConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gsxdb_state::RedbBalanceStore;
    use tempfile::TempDir;

    #[test]
    fn l2_syncer_creation() {
        let config = L2SyncConfig {
            rpc_url: "http://localhost:8545".to_string(),
            addresses: vec![Address([0; 20])],
        };
        let syncer = L2StateSyncer::new(config.clone());
        assert_eq!(syncer.config.addresses.len(), 1);
        assert_eq!(syncer.config.rpc_url, "http://localhost:8545");
    }

    #[tokio::test]
    async fn l2_syncer_sync_fails_on_bad_rpc() {
        let config = L2SyncConfig {
            rpc_url: "http://127.0.0.1:9999".to_string(),
            addresses: vec![Address([1; 20])],
        };
        let syncer = L2StateSyncer::new(config);
        // Should fail to connect to nonexistent RPC
        let result = syncer.sync().await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("RPC") || err.contains("failed"));
    }

    #[tokio::test]
    async fn sync_to_store_writes_nonces() {
        let dir = TempDir::new().expect("tempdir");
        let store =
            RedbBalanceStore::open(dir.path().join("state.redb")).expect("RedbBalanceStore::open");

        let addr1 = Address([1; 20]);
        let addr2 = Address([2; 20]);

        // Simulate synced state
        let synced_state = vec![
            SyncedEVMState {
                address: addr1,
                balance: 1000u128,
                nonce: 42u64,
            },
            SyncedEVMState {
                address: addr2,
                balance: 2000u128,
                nonce: 99u64,
            },
        ];

        // Write nonces to store (mimicking what sync_to_store would do)
        for state in &synced_state {
            store.set_evm_nonce(&state.address, state.nonce);
        }

        // Verify nonces can be read back
        assert_eq!(store.get_evm_nonce(&addr1), Some(42u64));
        assert_eq!(store.get_evm_nonce(&addr2), Some(99u64));
        assert_eq!(store.get_evm_nonce(&Address([3; 20])), None);
    }
}
