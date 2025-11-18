use crate::types::*;
use anyhow::Result;
use redis::aio::MultiplexedConnection;
use std::collections::HashMap;

pub async fn knn_one(
    conn: &mut MultiplexedConnection,
    model: &str,
    route: &str,
    vec_bytes: &[u8],
) -> Result<(Option<String>, Option<f32>)> {
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

pub async fn hgetex_resp_refresh(
    conn: &mut MultiplexedConnection,
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
        redis::Value::BulkString(_) | redis::Value::SimpleString(_) => as_string(&v),
        redis::Value::Nil => None,
        _ => None,
    })
}

pub async fn emit_metric(
    conn: &mut MultiplexedConnection,
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

fn parse_knn_one(reply: redis::Value) -> Result<(Option<String>, Option<f32>)> {
    fn s(v: &redis::Value) -> String {
        let raw = match v {
            redis::Value::BulkString(b) => String::from_utf8_lossy(b).to_string(),
            redis::Value::SimpleString(s) => s.clone(),
            redis::Value::VerbatimString { text, .. } => {
                String::from_utf8_lossy(text.as_bytes()).to_string()
            }
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
            if let redis::Value::Array(entries) = &top[2] {
                let mut i = 0;
                while i + 1 < entries.len() {
                    let name = s(&entries[i]);
                    let val = s(&entries[i + 1]);
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
            Ok((Some(key), dist))
        }
        _ => Ok((None, None)),
    }
}

pub async fn read_metrics(conn: &mut MultiplexedConnection, count: i64) -> Result<MetricsOut> {
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

pub async fn read_last_latencies(
    conn: &mut MultiplexedConnection,
    count: i64,
) -> Result<Vec<u128>> {
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
        redis::Value::VerbatimString { text, .. } => {
            String::from_utf8_lossy(text.as_bytes()).to_string()
        }
        redis::Value::Nil => String::new(),
        _ => format!("{:?}", v),
    }
}
