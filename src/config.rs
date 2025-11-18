//! Loads runtime configuration for latency-slayer service by reading required environment
//! variables, applying sensible defaults, and exposing the values through the
//! strongly typed `Config` struct.
use anyhow::Result;
use dotenvy::dotenv;

#[derive(Clone)]
pub struct Config {
    pub redis_url: String,
    pub openai_api_key: String,
    pub embed_dim: usize,
    pub ttl_secs: usize,
    pub threshold: f32,
    pub gen_model: String,
    pub miss_delay_ms: Option<u64>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        dotenv().ok(); // Load .env file if present
        let redis_url = std::env::var("REDIS_URL")
            .expect("REDIS_URL is required, e.g. rediss://default:<pwd>@<host>:<port>");
        let openai_api_key = std::env::var("OPENAI_API_KEY").expect("OPENAI_API_KEY is required");
        let embed_dim: usize = std::env::var("EMBED_DIM")
            .unwrap_or_else(|_| "1536".into())
            .parse()?;
        let ttl_secs: usize = std::env::var("RESP_TTL_SECS")
            .unwrap_or_else(|_| "86400".into())
            .parse()?;
        let threshold: f32 = std::env::var("CACHE_DISTANCE_THRESHOLD")
            .unwrap_or_else(|_| "0.10".into())
            .parse()?;
        let gen_model = std::env::var("GEN_MODEL").unwrap_or_else(|_| "gpt-5-mini".into());
        let miss_delay_ms = std::env::var("MISS_DELAY_MS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok());

        Ok(Config {
            redis_url,
            openai_api_key,
            embed_dim,
            ttl_secs,
            threshold,
            gen_model,
            miss_delay_ms,
        })
    }
}
