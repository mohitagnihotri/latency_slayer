// This file is part of the Latency Slayer project, which implements a Redis-based semantic cache
// for OpenAI chat completions, optimizing for low latency and high hit rates.
// Contributors: Mohit Agnihotri
// References: Redis Labs and the OpenAI documentation for embeddings and chat completions.

pub mod cache_utils;
pub mod config;
pub mod http;
pub mod openai_client;
pub mod redis_store;
pub mod types;

use crate::config::Config;
use crate::http::*;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

// ============================
// Main
// ============================
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
    let client = redis::Client::open(config.redis_url.clone())?;
    let state = Arc::new(AppState {
        redis: client,
        config,
    });

    let app = Router::new()
        .route("/", get(index_html))
        .route("/metrics", get(metrics_endpoint))
        .route("/metrics/fragment", get(metrics_fragment))
        .route("/metrics/sparkline", get(metrics_sparkline))
        .route("/chat", post(chat))
        .with_state(state);

    let addr = std::net::SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let localhost_hint = format!("http://localhost:{}", addr.port());
    println!(
        "Latency Slayer service is listening on http://{addr} (open {localhost_hint} in your browser)"
    );
    axum::serve(listener, app).await?;
    Ok(())
}
