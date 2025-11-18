use crate::config::Config;
use serde::{Deserialize, Serialize};

// ============================
// App State & Types
// ============================
#[derive(Clone)]
pub struct AppState {
    pub redis: redis::Client,
    pub config: Config,
}

#[derive(Deserialize, Clone)]
pub struct Message {
    pub(crate) role: String,
    pub(crate) content: String,
}

#[derive(Deserialize)]
pub struct ChatReq {
    pub(crate) user: Option<String>,
    pub(crate) model: Option<String>,
    pub(crate) route: Option<String>,
    pub(crate) messages: Vec<Message>,
    pub(crate) threshold: Option<f32>,
}

#[derive(Serialize)]
pub struct ChatResp {
    pub(crate) cached: bool,
    pub(crate) response: String,
    pub(crate) latency_ms: u128,
    pub(crate) hit_distance: Option<f32>,
    pub(crate) hit_key: Option<String>,
}

#[derive(Serialize, Default)]
pub struct MetricsOut {
    pub(crate) total: usize,
    pub(crate) hits: usize,
    pub(crate) hit_rate: f32,
    pub(crate) p50_ms: u128,
    pub(crate) p95_ms: u128,
}
