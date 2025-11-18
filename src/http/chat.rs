use super::AppState;
use crate::cache_utils::{canonicalize, f32s_to_bytes, fingerprint, normalize_tag, now_ms};
use crate::openai_client::{call_llm_openai, get_openai_embedding};
use crate::redis_store::{emit_metric, hgetex_resp_refresh, knn_one};
use crate::types::{ChatReq, ChatResp};
use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

pub async fn chat(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ChatReq>,
) -> Result<Json<ChatResp>, StatusCode> {
    let t0 = std::time::Instant::now();

    // Normalize tags to prevent query parser quirks
    let user = body.user.unwrap_or_else(|| "anon".into());
    let model_tag = normalize_tag(body.model.unwrap_or_else(|| "gpt_5_mini".into()));
    let route_tag = normalize_tag(body.route.unwrap_or_else(|| "default".into()));

    // Canonicalize prompt
    let prompt = canonicalize(&body.messages);
    let fp = fingerprint(&prompt, &model_tag, &route_tag);
    let key = format!("cache:{fp}");

    // 1) Embedding (OpenAI text-embedding-3-small -> 1536 dims)
    let emb = get_openai_embedding(&prompt, &state.config.openai_api_key)
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    if emb.len() != state.config.embed_dim {
        eprintln!(
            "embedding dim mismatch: got {} expected {}",
            emb.len(),
            state.config.embed_dim
        );
        return Err(StatusCode::BAD_GATEWAY);
    }
    let vec_bytes = f32s_to_bytes(&emb);

    // 2) KNN search for near-duplicate
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let (hit_key, hit_distance) = knn_one(&mut conn, &model_tag, &route_tag, &vec_bytes)
        .await
        .unwrap_or((None, None));

    let threshold = body.threshold.unwrap_or(state.config.threshold);

    if let (Some(hk), Some(dist)) = (hit_key.clone(), hit_distance) {
        if dist <= threshold {
            if let Ok(Some(resp)) = hgetex_resp_refresh(&mut conn, &hk, state.config.ttl_secs).await
            {
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
    if let Some(ms) = state.config.miss_delay_ms {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
    }

    // 3) Miss → call OpenAI Chat
    let response = call_llm_openai(
        &prompt,
        &state.config.gen_model,
        &state.config.openai_api_key,
    )
    .await
    .map_err(|_| StatusCode::BAD_GATEWAY)?;

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
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

        let _: redis::Value = redis::cmd("HSETEX")
            .arg(&key)
            .arg("EX")
            .arg(state.config.ttl_secs)
            .arg("FIELDS")
            .arg(1)
            .arg("resp")
            .arg(&response)
            .query_async(&mut conn)
            .await
            .map_err(|_| StatusCode::BAD_GATEWAY)?;

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
