use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub state_db_path: PathBuf,
    pub block_db_path: PathBuf,
    pub rpc_port: u16,
    pub metrics_port: u16,
    #[serde(default)]
    pub anchors: Vec<AnchorConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnchorConfig {
    pub chain_id: u64,
    pub key: String, // hex-encoded 32-byte key
}

impl Config {
    pub fn from_file(path: impl AsRef<std::path::Path>) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn from_env() -> Self {
        let state_db_path = std::env::var("STATE_DB_PATH")
            .unwrap_or_else(|_| "/data/gsxdb/state.redb".to_string())
            .into();
        let block_db_path = std::env::var("BLOCK_DB_PATH")
            .unwrap_or_else(|_| "/data/gsxdb/blocks.redb".to_string())
            .into();
        let rpc_port = std::env::var("RPC_PORT")
            .unwrap_or_else(|_| "8660".to_string())
            .parse()
            .expect("RPC_PORT must be a valid u16");
        let metrics_port = std::env::var("METRICS_PORT")
            .unwrap_or_else(|_| "9660".to_string())
            .parse()
            .expect("METRICS_PORT must be a valid u16");

        Self {
            state_db_path,
            block_db_path,
            rpc_port,
            metrics_port,
            anchors: vec![],
        }
    }
}
