use axum::{extract::State as AxumState, Json};
use suwappudb_bridge::AnchorDispatcher;
use suwappudb_state::Address;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

pub type AppState = Arc<Mutex<suwappudb_state::State>>;

#[derive(Clone)]
pub struct RpcHandler {
    state: AppState,
    dispatcher: Arc<Mutex<AnchorDispatcher>>,
}

impl RpcHandler {
    pub fn new(state: AppState, dispatcher: Arc<Mutex<AnchorDispatcher>>) -> Self {
        Self { state, dispatcher }
    }

    pub async fn handle(&self, method: &str, params: Vec<Value>) -> Value {
        match method {
            "suwappu_getBalance" => self.get_balance(&params).await,
            "suwappu_getCoinValue" => self.get_coin_value(&params).await,
            "suwappu_getStateRoot" => self.get_state_root().await,
            "suwappu_getBlock" => self.get_block(&params).await,
            "suwappu_getParity" => self.get_parity(&params).await,
            "suwappu_submitIntent" => self.submit_intent(&params).await,
            _ => json!({
                "error": "method not found",
                "code": -32601
            }),
        }
    }

    async fn get_balance(&self, params: &[Value]) -> Value {
        if params.is_empty() {
            return json!({ "error": "missing address parameter" });
        }
        match parse_address_param(&params[0]) {
            Some(addr) => {
                let state = self.state.lock().await;
                let slot = state.slot_of(&addr);
                let addr_hex = hex::encode(addr.0);
                json!({
                    "balance": slot.evm_balance().to_u128().to_string(),
                    "address": addr_hex,
                })
            }
            None => json!({ "error": "invalid address format" }),
        }
    }

    async fn get_coin_value(&self, params: &[Value]) -> Value {
        if params.is_empty() {
            return json!({ "error": "missing address parameter" });
        }
        match parse_address_param(&params[0]) {
            Some(addr) => {
                let state = self.state.lock().await;
                let slot = state.slot_of(&addr);
                let addr_hex = hex::encode(addr.0);
                json!({
                    "coin_value": slot.move_coin_value().to_u128().to_string(),
                    "address": addr_hex,
                })
            }
            None => json!({ "error": "invalid address format" }),
        }
    }

    async fn get_state_root(&self) -> Value {
        let state = self.state.lock().await;
        let tree = suwappudb_state::StateTree::from_state(&state);
        let root = tree.root();
        let root_hex = root
            .0
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        json!({
            "state_root": root_hex,
        })
    }

    async fn get_block(&self, _params: &[Value]) -> Value {
        // Placeholder — needs BlockStore access
        json!({ "error": "not yet implemented" })
    }

    async fn get_parity(&self, params: &[Value]) -> Value {
        if params.is_empty() {
            return json!({ "error": "missing height parameter" });
        }

        if let Some(height) = params[0].as_u64() {
            let dispatcher = self.dispatcher.lock().await;
            let result = dispatcher.parity_check(height);

            match result {
                suwappudb_bridge::ParityResult::Agreed { state_root } => {
                    let root_hex = state_root
                        .0
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect::<String>();
                    json!({
                        "parity": "agreed",
                        "state_root": root_hex,
                        "height": height
                    })
                }
                suwappudb_bridge::ParityResult::Disagreed { divergent, missing } => {
                    let divergent_json: Vec<_> = divergent
                        .iter()
                        .map(|(chain_id, root)| {
                            let root_hex = root
                                .0
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<String>();
                            json!({
                                "chain_id": chain_id.0,
                                "state_root": root_hex
                            })
                        })
                        .collect();

                    let missing_ids: Vec<u32> = missing.iter().map(|c| c.0).collect();

                    json!({
                        "parity": "disagreed",
                        "height": height,
                        "divergent": divergent_json,
                        "missing": missing_ids
                    })
                }
            }
        } else {
            json!({ "error": "height must be a valid u64" })
        }
    }

    async fn submit_intent(&self, _params: &[Value]) -> Value {
        // Placeholder — needs lane access
        json!({ "error": "not yet implemented" })
    }
}

// JSON-RPC request/response types
#[derive(serde::Deserialize)]
pub struct RpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub method: String,
    pub params: Vec<Value>,
    pub id: Value,
}

#[derive(serde::Serialize)]
pub struct RpcResponse {
    pub jsonrpc: String,
    pub result: Value,
    pub id: Value,
}

pub async fn rpc_handler(
    AxumState(handler): AxumState<RpcHandler>,
    Json(req): Json<RpcRequest>,
) -> Json<RpcResponse> {
    let result = handler.handle(&req.method, req.params).await;
    Json(RpcResponse {
        jsonrpc: "2.0".to_string(),
        result,
        id: req.id,
    })
}

/// **B2** — parse a JSON-RPC param into an [`Address`] without panicking
/// on bad input. Accepts an optional `0x` prefix and exactly 40 hex
/// chars (20 bytes). Returns `None` on any malformed shape; the
/// caller emits a typed error response.
///
/// Pre-B2 this lived inline as `bytes.try_into().unwrap()` after a
/// length-20 check. That was logically safe but kept an unwrap on an
/// untrusted-input handler; the workspace lint posture forbids it.
fn parse_address_param(value: &Value) -> Option<Address> {
    let raw = value.as_str()?;
    let hex_part = raw.strip_prefix("0x").unwrap_or(raw);
    if hex_part.len() != 40 {
        return None;
    }
    let bytes = hex::decode(hex_part).ok()?;
    let arr: [u8; 20] = bytes.try_into().ok()?;
    Some(Address(arr))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn parse_address_accepts_canonical_hex() {
        let v: Value = Value::String("aa".repeat(20));
        assert_eq!(parse_address_param(&v), Some(Address([0xAA; 20])));
    }

    #[test]
    fn parse_address_accepts_0x_prefix() {
        let v: Value = Value::String(format!("0x{}", "bb".repeat(20)));
        assert_eq!(parse_address_param(&v), Some(Address([0xBB; 20])));
    }

    #[test]
    fn parse_address_rejects_wrong_length() {
        let v: Value = Value::String("aabbcc".to_string());
        assert_eq!(parse_address_param(&v), None);
    }

    #[test]
    fn parse_address_rejects_non_hex() {
        let v: Value = Value::String("zz".repeat(20));
        assert_eq!(parse_address_param(&v), None);
    }

    #[test]
    fn parse_address_rejects_non_string() {
        assert_eq!(parse_address_param(&Value::Null), None);
        assert_eq!(parse_address_param(&serde_json::json!(42)), None);
        assert_eq!(parse_address_param(&serde_json::json!([])), None);
    }
}
