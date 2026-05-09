//! L2 state syncer — reads balance and nonce state from op-reth.

use gsxdb_state::Address;

/// Configuration for L2 state synchronization.
#[derive(Debug, Clone)]
pub struct L2SyncConfig {
    /// JSON-RPC endpoint URL for op-reth (e.g., http://localhost:8545)
    pub rpc_url: String,
    /// Addresses to sync balance and nonce for
    pub addresses: Vec<Address>,
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

    /// Sync balance and nonce for all configured addresses.
    /// Reads via `eth_getBalance` and `eth_getTransactionCount` from op-reth.
    ///
    /// # Returns
    ///
    /// `Ok(())` on successful sync, or an error message.
    ///
    /// # Phase-1 Note
    ///
    /// This is a placeholder. Real implementation calls op-reth JSON-RPC:
    /// - `eth_getBalance(address, "latest")` → wei as hex string
    /// - `eth_getTransactionCount(address, "latest")` → nonce as hex
    pub async fn sync(&self) -> Result<(), String> {
        // TODO: Implement eth_getBalance and eth_getTransactionCount calls
        // For phase-1, this is a placeholder.
        tracing::info!(
            "L2StateSyncer configured with {} addresses at {}",
            self.config.addresses.len(),
            self.config.rpc_url
        );
        Ok(())
    }

    /// Get reference to the sync configuration.
    pub fn config(&self) -> &L2SyncConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    async fn l2_syncer_sync_placeholder() {
        let config = L2SyncConfig {
            rpc_url: "http://localhost:8545".to_string(),
            addresses: vec![Address([0; 20])],
        };
        let syncer = L2StateSyncer::new(config);
        assert!(syncer.sync().await.is_ok());
    }
}
