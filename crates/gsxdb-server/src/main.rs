mod config;
mod rpc;

use axum::{
    extract::State as AxumState,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use config::Config;
use gsxdb_state::{RedbBalanceStore, State};
use rpc::RpcHandler;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    // Load config
    let config = if let Ok(path) = std::env::var("CONFIG_PATH") {
        Config::from_file(path)?
    } else {
        Config::from_env()
    };

    tracing::info!("Starting gsxdb-server on port {}", config.rpc_port);
    tracing::info!("State DB: {}", config.state_db_path.display());
    tracing::info!("Block DB: {}", config.block_db_path.display());

    // Initialize state with redb backend
    let state_db = RedbBalanceStore::open(&config.state_db_path)?;
    let state = Arc::new(Mutex::new(State::with_store(Box::new(state_db))));

    // Create RPC handler
    let rpc_handler = RpcHandler::new(state.clone());

    // Build router
    let app = Router::new()
        .route("/health", get(health_check))
        .route("/metrics", get(metrics_endpoint))
        .route("/rpc", post(rpc::rpc_handler))
        .with_state(rpc_handler)
        .fallback(not_found);

    // Start server
    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.rpc_port))
        .await?;

    tracing::info!("Server listening on 0.0.0.0:{}", config.rpc_port);

    axum::serve(listener, app).await?;

    Ok(())
}

async fn health_check() -> StatusCode {
    StatusCode::OK
}

async fn metrics_endpoint() -> String {
    // Placeholder for Prometheus metrics
    "# TYPE gsxdb_info gauge\ngsxdb_info{version=\"0.0.1\"} 1\n".to_string()
}

async fn not_found() -> (StatusCode, &'static str) {
    (StatusCode::NOT_FOUND, "Not Found")
}
