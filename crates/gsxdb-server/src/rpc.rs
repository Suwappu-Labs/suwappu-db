use axum::{extract::State as AxumState, Json};
use gsxdb_bridge::AnchorDispatcher;
use gsxdb_state::Address;
use serde_json::{json, Value};
use std::sync::Arc;
use tokio::sync::Mutex;

pub type AppState = Arc<Mutex<gsxdb_state::State>>;

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
            "gsx_getBalance" => self.get_balance(&params).await,
            "gsx_getCoinValue" => self.get_coin_value(&params).await,
            "gsx_getStateRoot" => self.get_state_root().await,
            "gsx_getBlock" => self.get_block(&params).await,
            "gsx_getParity" => self.get_parity(&params).await,
            "gsx_submitIntent" => self.submit_intent(&params).await,
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

        if let Some(addr_str) = params[0].as_str() {
            if addr_str.len() == 40 {
                // Parse hex address
                if let Ok(bytes) = hex::decode(&addr_str) {
                    if bytes.len() == 20 {
                        let addr = Address(bytes.try_into().unwrap());
                        let state = self.state.lock().await;
                        let slot = state.slot_of(&addr);
                        return json!({
                            "balance": slot.evm_balance().to_u128().to_string(),
                            "address": addr_str
                        });
                    }
                }
            }
        }

        json!({ "error": "invalid address format" })
    }

    async fn get_coin_value(&self, params: &[Value]) -> Value {
        if params.is_empty() {
            return json!({ "error": "missing address parameter" });
        }

        if let Some(addr_str) = params[0].as_str() {
            if addr_str.len() == 40 {
                if let Ok(bytes) = hex::decode(&addr_str) {
                    if bytes.len() == 20 {
                        let addr = Address(bytes.try_into().unwrap());
                        let state = self.state.lock().await;
                        let slot = state.slot_of(&addr);
                        return json!({
                            "coin_value": slot.move_coin_value().to_u128().to_string(),
                            "address": addr_str
                        });
                    }
                }
            }
        }

        json!({ "error": "invalid address format" })
    }

    async fn get_state_root(&self) -> Value {
        let state = self.state.lock().await;
        let tree = gsxdb_state::StateTree::from_state(&*state);
        let root = tree.root();
        let root_hex = root.0.iter().map(|b| format!("{:02x}", b)).collect::<String>();
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
                gsxdb_bridge::ParityResult::Agreed { state_root } => {
                    let root_hex = state_root.0.iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();
                    json!({
                        "parity": "agreed",
                        "state_root": root_hex,
                        "height": height
                    })
                }
                gsxdb_bridge::ParityResult::Disagreed { divergent, missing } => {
                    let divergent_json: Vec<_> = divergent.iter()
                        .map(|(chain_id, root)| {
                            let root_hex = root.0.iter()
                                .map(|b| format!("{:02x}", b))
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
