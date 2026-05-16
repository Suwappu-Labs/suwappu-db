//! gsxdb-server — HTTP server for state queries and RPC endpoints.
//!
//! Exposes JSON-RPC methods for balance queries, state root, and parity checks.

use axum::{
    extract::State as AxumState,
    http::StatusCode,
    middleware,
    response::Json,
    routing::{get, post},
    Router,
};
use gsxdb_server::{bearer_auth, BearerAuthConfig};
use gsxdb_state::{Address, State};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

/// Server configuration.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    /// Port to listen on.
    pub port: u16,
    /// Metrics port.
    pub metrics_port: u16,
}

/// Application state (wrapped in Arc<Mutex<>> for sharing across handlers).
struct AppState {
    config: ServerConfig,
    state: Mutex<State>,
}

/// Health check response.
#[derive(Serialize)]
struct HealthResponse {
    status: String,
}

/// Generic JSON-RPC request.
#[derive(Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    method: String,
    params: serde_json::Value,
    id: serde_json::Value,
}

/// Generic JSON-RPC response.
#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
    id: serde_json::Value,
}

/// JSON-RPC error object.
#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

/// Handler: GET /health
async fn health() -> (StatusCode, Json<HealthResponse>) {
    (
        StatusCode::OK,
        Json(HealthResponse {
            status: "ok".to_string(),
        }),
    )
}

/// Handler: GET /metrics (placeholder)
async fn metrics() -> (StatusCode, String) {
    (
        StatusCode::OK,
        "# HELP gsxdb_info GSX-DB server info\n".to_string(),
    )
}

/// **C4 deprecation alias** — old `/rpc` path. Logs a warning on
/// every hit and routes through the same handler as `/v1/rpc`.
/// Will be removed in the next minor bump after `v0.1.0-pre`.
async fn rpc_deprecated_alias(
    state: AxumState<Arc<AppState>>,
    req: Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    tracing::warn!(
        target: "gsxdb_server::rpc",
        "POST /rpc is deprecated; use /v1/rpc. method={:?}",
        req.0.method
    );
    rpc(state, req).await
}

/// Handler: POST /v1/rpc (canonical) and POST /rpc (deprecated alias).
async fn rpc(
    AxumState(state): AxumState<Arc<AppState>>,
    Json(req): Json<JsonRpcRequest>,
) -> (StatusCode, Json<JsonRpcResponse>) {
    let response = match req.method.as_str() {
        "gsx_getBalance" => {
            if let serde_json::Value::Array(params) = req.params {
                if params.len() >= 1 {
                    if let Some(addr_str) = params[0].as_str() {
                        match parse_address(addr_str) {
                            Ok(addr) => {
                                let state_guard = state.state.lock().await;
                                let balance = state_guard.balance_of(&addr);
                                JsonRpcResponse {
                                    jsonrpc: "2.0".to_string(),
                                    result: Some(json!({
                                        "address": addr_str,
                                        "balance": balance.0.to_string(),
                                    })),
                                    error: None,
                                    id: req.id,
                                }
                            }
                            Err(_) => JsonRpcResponse {
                                jsonrpc: "2.0".to_string(),
                                result: None,
                                error: Some(JsonRpcError {
                                    code: -32602,
                                    message: "Invalid address format".to_string(),
                                }),
                                id: req.id,
                            },
                        }
                    } else {
                        JsonRpcResponse {
                            jsonrpc: "2.0".to_string(),
                            result: None,
                            error: Some(JsonRpcError {
                                code: -32602,
                                message: "Invalid parameter: address must be a string".to_string(),
                            }),
                            id: req.id,
                        }
                    }
                } else {
                    JsonRpcResponse {
                        jsonrpc: "2.0".to_string(),
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32602,
                            message: "Missing required parameter: address".to_string(),
                        }),
                        id: req.id,
                    }
                }
            } else {
                JsonRpcResponse {
                    jsonrpc: "2.0".to_string(),
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32602,
                        message: "Invalid parameters: must be an array".to_string(),
                    }),
                    id: req.id,
                }
            }
        }
        "gsx_getStateRoot" => {
            let state_guard = state.state.lock().await;
            // Placeholder: compute hash of all state entries
            // TODO: Wire to actual state tree commitment after S6 integration
            let entries = state_guard.entries();
            let mut hasher = blake3::Hasher::new();
            for (addr, _slot) in entries {
                hasher.update(&addr.0);
            }
            let root = hasher.finalize();
            JsonRpcResponse {
                jsonrpc: "2.0".to_string(),
                result: Some(json!({
                    "state_root": format!("0x{}", hex::encode(root.as_bytes())),
                })),
                error: None,
                id: req.id,
            }
        }
        _ => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            result: None,
            error: Some(JsonRpcError {
                code: -32601,
                message: "Method not found".to_string(),
            }),
            id: req.id,
        },
    };

    (StatusCode::OK, Json(response))
}

/// Parse an address string (0x-prefixed hex or raw hex) into Address.
fn parse_address(s: &str) -> Result<Address, String> {
    let hex_str = if s.starts_with("0x") { &s[2..] } else { s };

    if hex_str.len() != 40 {
        return Err("Address must be 20 bytes (40 hex chars)".to_string());
    }

    let bytes = hex::decode(hex_str).map_err(|e| format!("Invalid hex: {}", e))?;
    let mut addr = [0u8; 20];
    addr.copy_from_slice(&bytes);
    Ok(Address(addr))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt::init();

    let config = ServerConfig {
        port: 8660,
        metrics_port: 9660,
    };

    info!("Starting gsxdb-server on port {}", config.port);

    let state = Arc::new(AppState {
        config: config.clone(),
        state: Mutex::new(State::default()),
    });

    // Build router.
    //
    // **C4 — /v1/ versioning.** Canonical JSON-RPC route is now
    // `/v1/rpc`. Root `/rpc` is preserved as a deprecation alias for
    // one cycle (logged on each hit). `/health` and `/metrics` are
    // not versioned — they're operational endpoints whose contract
    // is set by Kubernetes probes / Prometheus and won't move.
    let app = Router::new()
        .route("/health", get(health))
        .route("/metrics", get(metrics))
        .route("/v1/rpc", post(rpc))
        .route("/rpc", post(rpc_deprecated_alias))
        .with_state(state);

    // Run server
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    info!("Server listening on 0.0.0.0:{}", config.port);

    axum::serve(listener, app).await?;

    Ok(())
}
