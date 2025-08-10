// This file is part of the Latency Slayer project (Redis AI Challenge), which implements a Redis-based semantic cache
// for OpenAI chat completions, optimizing for low latency and high hit rates.
// Contributors: Mohit Agnihotri
// References: Redis Labs and the OpenAI documentation for embeddings and chat completions.

pub mod types;

use crate::types::*;
use axum::{
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{collections::HashMap, sync::Arc};

// ============================
// Main
// ============================
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Env
    let redis_url = std::env::var("REDIS_URL")
        .expect("REDIS_URL is required, e.g. rediss://default:<pwd>@<host>:<port>");
    let embed_dim: usize = std::env::var("EMBED_DIM")
        .unwrap_or_else(|_| "1536".into())
        .parse()?;
    let ttl_secs: usize = std::env::var("RESP_TTL_SECS")
        .unwrap_or_else(|_| "86400".into())
        .parse()?;
    let threshold: f32 = std::env::var("CACHE_DISTANCE_THRESHOLD")
        .unwrap_or_else(|_| "0.10".into())
        .parse()?;
    let gen_model = std::env::var("GEN_MODEL").unwrap_or_else(|_| "gpt-4o-mini".into());

    let client = redis::Client::open(redis_url)?;
    let state = Arc::new(AppState {
        redis: client,
        embed_dim,
        ttl_secs,
        threshold,
        gen_model,
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
    println!("Latency Slayer listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

// ============================
// Handlers
// ============================
async fn chat(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    Json(body): Json<ChatReq>,
) -> Result<Json<ChatResp>, axum::http::StatusCode> {
    let t0 = std::time::Instant::now();

    // Normalize tags to prevent query parser quirks
    let user = body.user.unwrap_or_else(|| "anon".into());
    let model_tag = normalize_tag(body.model.unwrap_or_else(|| "gpt_4o_mini".into()));
    let route_tag = normalize_tag(body.route.unwrap_or_else(|| "default".into()));

    // Canonicalize prompt
    let prompt = canonicalize(&body.messages);
    let fp = fingerprint(&prompt, &model_tag, &route_tag);
    let key = format!("cache:{fp}");

    // 1) Embedding (OpenAI text-embedding-3-small -> 1536 dims)
    let emb = get_openai_embedding(&prompt)
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;
    if emb.len() != state.embed_dim {
        eprintln!(
            "embedding dim mismatch: got {} expected {}",
            emb.len(),
            state.embed_dim
        );
        return Err(axum::http::StatusCode::BAD_GATEWAY);
    }
    let vec_bytes = f32s_to_bytes(&emb);

    // 2) KNN search for near-duplicate
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

    let (hit_key, hit_distance) = knn_one(&mut conn, &model_tag, &route_tag, &vec_bytes)
        .await
        .unwrap_or((None, None));

    let threshold = body.threshold.unwrap_or(state.threshold);

    if let (Some(hk), Some(dist)) = (hit_key.clone(), hit_distance) {
        if dist <= threshold {
            if let Ok(Some(resp)) = hgetex_resp_refresh(&mut conn, &hk, state.ttl_secs).await {
                emit_metric(&mut conn, true, t0.elapsed(), 0).await.ok();
                return Ok(Json(ChatResp {
                    cached: true,
                    response: resp,
                    latency_ms: t0.elapsed().as_millis(),
                    hit_distance: Some(dist),
                    hit_key: Some(hk),
                }));
            }
        }
    }

    // Optional: simulate slow LLM for demo (env MISS_DELAY_MS)
    // if let Some(ms) = std::env::var("MISS_DELAY_MS")
    //     .ok()
    //     .and_then(|s| s.parse::<u64>().ok())
    // {
    //     tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    // }

    // 3) Miss → call OpenAI Chat
    let response = call_llm_openai(&prompt, &state.gen_model)
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

    // 4) Store fields (embedding w/o TTL; response with field-level TTL)
    {
        let mut pipe = redis::pipe();
        pipe.hset(&key, "prompt", &prompt)
            .hset(&key, "model", &model_tag)
            .hset(&key, "route", &route_tag)
            .hset(&key, "user", &user)
            .hset(&key, "created_at", now_ms().to_string())
            .hset(&key, "embedding", vec_bytes.clone());
        let _: redis::Value = pipe
            .query_async(&mut conn)
            .await
            .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

        let _: redis::Value = redis::cmd("HSETEX")
            .arg(&key)
            .arg("EX")
            .arg(state.ttl_secs)
            .arg("FIELDS")
            .arg(1)
            .arg("resp")
            .arg(&response)
            .query_async(&mut conn)
            .await
            .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

        emit_metric(&mut conn, false, t0.elapsed(), 0).await.ok();
    }

    Ok(Json(ChatResp {
        cached: false,
        response,
        latency_ms: t0.elapsed().as_millis(),
        hit_distance: None,
        hit_key: Some(key),
    }))
}

async fn metrics_endpoint(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<Json<MetricsOut>, axum::http::StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

    let metrics = read_metrics(&mut conn, 200).await.unwrap_or_default();
    Ok(Json(metrics))
}

async fn metrics_fragment(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<axum::http::Response<String>, axum::http::StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

    let m = read_metrics(&mut conn, 200).await.unwrap_or_default();
    let hit_pct = m.hit_rate * 100.0;

    let html = format!(
        r#"
<style>
  .tbl {{ width:100%; border-collapse: collapse; border:1px solid #d1d5db; border-radius: 8px; overflow:hidden; }}
  .tbl th, .tbl td {{ padding:10px 12px; border:1px solid #e5e7eb; text-align:left; }}
  .tbl thead th {{ font-size: 0.9rem; color:#374151; font-weight:700; background:#f3f4f6; }}
  .tbl td.num {{ text-align:right; font-variant-numeric: tabular-nums; }}
  .bar {{ height:8px; background:#eef2f7; border-radius:6px; overflow:hidden; margin-top:6px; }}
  .bar > div {{ height:100%; background:#10b981; }}
</style>
<table class=\"tbl\">
  <thead>
    <tr><th>Metric</th><th>Value</th></tr>
  </thead>
  <tbody>
    <tr><td>Requests</td><td class=\"num\">{}</td></tr>
    <tr><td>Hit rate</td><td class=\"num\">{:.1}%<div class=\"bar\"><div style=\"width:{:.1}%\"></div></div></td></tr>
    <tr><td>p50 (ms)</td><td class=\"num\">{}</td></tr>
    <tr><td>p95 (ms)</td><td class=\"num\">{}</td></tr>
  </tbody>
</table>
"#,
        m.total, hit_pct, hit_pct, m.p50_ms, m.p95_ms
    );

    Ok(axum::http::Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html)
        .unwrap())
}

async fn metrics_sparkline(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> Result<axum::http::Response<String>, axum::http::StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| axum::http::StatusCode::BAD_GATEWAY)?;

    let data = read_last_latencies(&mut conn, 20).await.unwrap_or_default();
    let w: f32 = 740.0; // viewBox width
    let h: f32 = 120.0; // viewBox height
    let pad: f32 = 12.0;

    let html = if data.len() >= 1 {
        let max_v = *data.iter().max().unwrap_or(&1) as f32;
        let span = if data.len() > 1 {
            (w - 2.0 * pad) / ((data.len() - 1) as f32)
        } else {
            0.0
        };
        let mut pts = String::new();
        let mut last_xy = (pad, h - pad);
        for (i, v) in data.iter().enumerate() {
            let x = pad + (i as f32) * span;
            let y = h - pad - ((*v as f32 / max_v) * (h - 2.0 * pad));
            pts.push_str(&format!("{:.1},{:.1} ", x, y));
            last_xy = (x, y);
        }
        format!(
            "<div id=\"spark\">\
<svg viewBox=\"0 0 {w} {h}\" width=\"100%\" height=\"140\" xmlns=\"http://www.w3.org/2000/svg\">\
  <rect x=\"0\" y=\"0\" width=\"{w}\" height=\"{h}\" fill=\"white\" stroke=\"#e5e7eb\" />\
  <polyline points=\"{pts}\" fill=\"none\" stroke=\"#2563eb\" stroke-width=\"2\" stroke-linejoin=\"round\" stroke-linecap=\"round\"/>\
  <circle cx=\"{cx}\" cy=\"{cy}\" r=\"3.5\" fill=\"#2563eb\" />\
</svg>\
</div>",
            w = w, h = h, pts = pts.trim(), cx = last_xy.0, cy = last_xy.1
        )
    } else {
        "<div id=\"spark\"><em class=\"muted\">No data yet</em></div>".to_string()
    };

    Ok(axum::http::Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html)
        .unwrap())
}

async fn index_html() -> axum::http::Response<String> {
    let html = r#"<!doctype html>
<html lang=\"en\">
<head>
  <meta charset=\"utf-8\" />
  <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\" />
  <title>Latency Slayer Dashboard</title>
  <script src=\"https://unpkg.com/htmx.org@1.9.12\" integrity=\"sha384-IQOtF1RZb5/Cd8tQz6qqn9qJbTtF5Vle0VjyFoP3n3oD8uH3Qf6P2kF0B1EM6w7U\" crossorigin=\"anonymous\"></script>
  <style>
    body { font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, Arial; margin: 24px; }
    .card { border: 1px solid #e5e7eb; border-radius: 16px; padding: 16px; max-width: 720px; box-shadow: 0 2px 8px rgba(0,0,0,0.04); }
    .kpi { display:flex; gap:16px; }
    .kpi div { flex:1; background:#f9fafb; border-radius:12px; padding:12px; text-align:center; }
    .muted { color:#6b7280; }
    code { background:#f3f4f6; padding:2px 6px; border-radius:6px; }
  </style>
</head>
<body>
  <h1>Latency Slayer <span class=\"muted\">(Redis 8 semantic cache)</span></h1>
  <div class=\"card\">
    <div id=\"kpis\" class=\"kpi\" hx-get=\"/metrics/fragment\" hx-trigger=\"load, every 2s\" hx-swap=\"innerHTML\">
      <div><div class=\"muted\">Requests</div><div>–</div></div>
      <div><div class=\"muted\">Hit rate</div><div>–</div></div>
      <div><div class=\"muted\">p50 (ms)</div><div>–</div></div>
      <div><div class=\"muted\">p95 (ms)</div><div>–</div></div>
    </div>
    <p class=\"muted\" style=\"margin-top:12px\">Auto-refreshes every 2s. Open DevTools → Network to see the HTMX calls.</p>
  </div>
</body>
</html>"#;
    axum::http::Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
}

// ============================
// Redis helpers & parsing
// ============================
async fn knn_one(
    conn: &mut redis::aio::MultiplexedConnection,
    model: &str,
    route: &str,
    vec_bytes: &[u8],
) -> anyhow::Result<(Option<String>, Option<f32>)> {
    // Use built-in __embedding_score
    let query = format!(
        "(@model:{{{}}} @route:{{{}}})=>[KNN 1 @embedding $vec]",
        model, route
    );

    let reply: redis::Value = redis::cmd("FT.SEARCH")
        .arg("idx:latency_slayer")
        .arg(query)
        .arg("PARAMS")
        .arg(2)
        .arg("vec")
        .arg(vec_bytes)
        .arg("SORTBY")
        .arg("__embedding_score")
        .arg("RETURN")
        .arg(1)
        .arg("__embedding_score")
        .arg("DIALECT")
        .arg(2)
        .query_async(conn)
        .await?;

    parse_knn_one(reply)
}

async fn hgetex_resp_refresh(
    conn: &mut redis::aio::MultiplexedConnection,
    key: &str,
    ttl: usize,
) -> redis::RedisResult<Option<String>> {
    let v: redis::Value = redis::cmd("HGETEX")
        .arg(key)
        .arg("EX")
        .arg(ttl)
        .arg("FIELDS")
        .arg(1)
        .arg("resp")
        .query_async(conn)
        .await?;

    fn as_string(v: &redis::Value) -> Option<String> {
        match v {
            redis::Value::BulkString(b) => Some(String::from_utf8_lossy(b).to_string()),
            redis::Value::SimpleString(s) => Some(s.clone()),
            redis::Value::VerbatimString { text, .. } => Some(text.clone()),
            _ => None,
        }
        .map(|s| s.trim_matches('"').to_string())
    }

    Ok(match v {
        redis::Value::Array(a) => {
            if a.is_empty() {
                None
            } else {
                as_string(&a[0])
            }
        }
        redis::Value::BulkString(_)
        | redis::Value::SimpleString(_)
        | redis::Value::VerbatimString { .. } => as_string(&v),
        redis::Value::Nil => None,
        _ => None,
    })
}

async fn emit_metric(
    conn: &mut redis::aio::MultiplexedConnection,
    hit: bool,
    dt: std::time::Duration,
    tokens_saved: i32,
) -> redis::RedisResult<()> {
    let _: String = redis::cmd("XADD")
        .arg("analytics:cache")
        .arg("*")
        .arg("hit")
        .arg(if hit { "1" } else { "0" })
        .arg("latency_ms")
        .arg(dt.as_millis().to_string())
        .arg("tokens_saved")
        .arg(tokens_saved.to_string())
        .query_async(conn)
        .await?;
    Ok(())
}

fn parse_knn_one(reply: redis::Value) -> anyhow::Result<(Option<String>, Option<f32>)> {
    fn s(v: &redis::Value) -> String {
        let raw = match v {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
            redis::Value::SimpleString(s) => s.clone(),
            redis::Value::VerbatimString { text, .. } => text.clone(),
            _ => String::new(),
        };
        raw.trim_matches('"').to_string()
    }

    match reply {
        redis::Value::Array(top) => {
            if top.len() < 3 {
                return Ok((None, None));
            }
            let key = s(&top[1]);
            if key.is_empty() {
                return Ok((None, None));
            }

            let mut dist: Option<f32> = None;
            match &top[2] {
                redis::Value::Array(v) => {
                    let mut i = 0;
                    while i + 1 < v.len() {
                        let name = s(&v[i]);
                        let val = s(&v[i + 1]);
                        if name == "__embedding_score" {
                            if let Ok(mut d) = val.parse::<f32>() {
                                if d < 0.0 {
                                    d = 0.0;
                                }
                                dist = Some(d);
                            }
                        }
                        i += 2;
                    }
                }
                redis::Value::Map(entries) => {
                    for (k, vv) in entries.iter() {
                        let name = s(k);
                        let val = s(vv);
                        if name == "__embedding_score" {
                            if let Ok(mut d) = val.parse::<f32>() {
                                if d < 0.0 {
                                    d = 0.0;
                                }
                                dist = Some(d);
                            }
                        }
                    }
                }
                _ => {}
            }
            Ok((Some(key), dist))
        }
        _ => Ok((None, None)),
    }
}

// ============================
// Metrics reader
// ============================
async fn read_metrics(
    conn: &mut redis::aio::MultiplexedConnection,
    count: i64,
) -> anyhow::Result<MetricsOut> {
    // XREVRANGE analytics:cache + - COUNT <count>
    let reply: redis::Value = redis::cmd("XREVRANGE")
        .arg("analytics:cache")
        .arg("+")
        .arg("-")
        .arg("COUNT")
        .arg(count)
        .query_async(conn)
        .await?;

    let mut hits = 0usize;
    let mut total = 0usize;
    let mut latencies: Vec<u128> = Vec::new();

    if let redis::Value::Array(entries) = reply {
        for entry in entries {
            if let redis::Value::Array(pair) = entry {
                if pair.len() != 2 {
                    continue;
                }
                if let redis::Value::Array(fields) = &pair[1] {
                    let mut map: HashMap<String, String> = HashMap::new();
                    let mut i = 0;
                    while i + 1 < fields.len() {
                        let k = val_to_string(&fields[i]);
                        let v = val_to_string(&fields[i + 1]);
                        map.insert(k, v);
                        i += 2;
                    }
                    total += 1;
                    if map.get("hit").map(|s| s == "1").unwrap_or(false) {
                        hits += 1;
                    }
                    if let Some(ms) = map.get("latency_ms").and_then(|s| s.parse::<u128>().ok()) {
                        latencies.push(ms);
                    }
                }
            }
        }
    }

    latencies.sort_unstable();
    let p = |q: f64| -> u128 {
        if latencies.is_empty() {
            return 0;
        }
        let idx = ((latencies.len() as f64 - 1.0) * q).round() as usize;
        latencies[idx]
    };

    let mut out = MetricsOut::default();
    out.total = total;
    out.hits = hits;
    out.hit_rate = if total == 0 {
        0.0
    } else {
        hits as f32 / total as f32
    };
    out.p50_ms = p(0.50);
    out.p95_ms = p(0.95);
    Ok(out)
}

async fn read_last_latencies(
    conn: &mut redis::aio::MultiplexedConnection,
    count: i64,
) -> anyhow::Result<Vec<u128>> {
    let reply: redis::Value = redis::cmd("XREVRANGE")
        .arg("analytics:cache")
        .arg("+")
        .arg("-")
        .arg("COUNT")
        .arg(count)
        .query_async(conn)
        .await?;

    let mut out: Vec<u128> = Vec::new();
    if let redis::Value::Array(entries) = reply {
        for entry in entries {
            if let redis::Value::Array(pair) = entry {
                if pair.len() != 2 {
                    continue;
                }
                if let redis::Value::Array(fields) = &pair[1] {
                    let mut map: HashMap<String, String> = HashMap::new();
                    let mut i = 0;
                    while i + 1 < fields.len() {
                        let k = val_to_string(&fields[i]);
                        let v = val_to_string(&fields[i + 1]);
                        map.insert(k, v);
                        i += 2;
                    }
                    if let Some(ms) = map.get("latency_ms").and_then(|s| s.parse::<u128>().ok()) {
                        out.push(ms);
                    }
                }
            }
        }
    }
    // XREVRANGE returns newest first; reverse to chronological order
    out.reverse();
    // Keep only the last N in correct order
    if out.len() > (count as usize) {
        let start = out.len() - count as usize;
        out = out[start..].to_vec();
    }
    Ok(out)
}

fn val_to_string(v: &redis::Value) -> String {
    match v {
        redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
        redis::Value::SimpleString(s) => s.clone(),
        redis::Value::VerbatimString { text, .. } => text.clone(),
        redis::Value::Nil => String::new(),
        _ => format!("{:?}", v),
    }
}

// ============================
// Utils
// ============================
fn canonicalize(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("{}:{}", m.role.trim(), m.content.trim()))
        .collect::<Vec<_>>()
        .join(
            "
",
        )
}

fn fingerprint(prompt: &str, model: &str, route: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(prompt.as_bytes());
    hasher.update(model.as_bytes());
    hasher.update(route.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn normalize_tag(s: String) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    out
}

fn f32s_to_bytes(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_ne_bytes());
    }
    out
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

// ============================
// OpenAI: Embeddings & Chat
// ============================
async fn get_openai_embedding(text: &str) -> anyhow::Result<Vec<f32>> {
    #[derive(serde::Serialize)]
    struct EmbReq<'a> {
        model: &'a str,
        input: &'a str,
    }
    #[derive(serde::Deserialize)]
    struct EmbResp {
        data: Vec<Item>,
    }
    #[derive(serde::Deserialize)]
    struct Item {
        embedding: Vec<f32>,
    }

    let api_key = std::env::var("OPENAI_API_KEY")?;
    let client = reqwest::Client::new();

    let resp: EmbResp = client
        .post("https://api.openai.com/v1/embeddings")
        .bearer_auth(api_key)
        .json(&EmbReq {
            model: "text-embedding-3-small",
            input: text,
        })
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    Ok(resp
        .data
        .into_iter()
        .next()
        .map(|i| i.embedding)
        .unwrap_or_default())
}

async fn call_llm_openai(prompt: &str, model: &str) -> anyhow::Result<String> {
    #[derive(serde::Serialize)]
    struct Msg {
        role: &'static str,
        content: String,
    }
    #[derive(serde::Serialize)]
    struct ChatReq {
        model: String,
        messages: Vec<Msg>,
    }
    #[derive(Deserialize)]
    struct ChatResp {
        choices: Vec<Choice>,
        usage: Option<Usage>,
    }
    #[derive(Deserialize)]
    struct Choice {
        message: ChoiceMsg,
    }
    #[derive(Deserialize)]
    struct ChoiceMsg {
        content: String,
    }
    #[derive(Deserialize)]
    struct Usage {
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
    }

    let api_key = std::env::var("OPENAI_API_KEY")?;
    let client = reqwest::Client::new();

    let req = ChatReq {
        model: model.to_string(),
        messages: vec![Msg {
            role: "user",
            content: prompt.to_string(),
        }],
    };

    let resp: ChatResp = client
        .post("https://api.openai.com/v1/chat/completions")
        .bearer_auth(api_key)
        .json(&req)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let text = resp
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .unwrap_or_default();
    Ok(text)
}
