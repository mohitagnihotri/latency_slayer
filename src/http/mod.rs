use crate::config::Config;
use redis::Client;

pub mod chat;
pub mod dashboard;
pub mod metrics;

pub use chat::chat;
pub use dashboard::index_html;
pub use metrics::{metrics_endpoint, metrics_fragment, metrics_sparkline};

#[derive(Clone)]
pub struct AppState {
    pub redis: Client,
    pub config: Config,
}
