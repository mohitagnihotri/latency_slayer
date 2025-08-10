# Latency Slayer — Redis 8 Semantic Cache for LLMs

Latency Slayer is a tiny Rust web service that **caches LLM answers semantically** using Redis 8.  
It embeds incoming prompts, does a vector **KNN** lookup (HNSW, cosine), and returns cached answers on near-duplicates.  
It also uses Redis 8 **field-level TTL** for answers and a **stream** for real-time metrics (hit-rate, p50, p95), with a simple HTMX dashboard.

https://dev.to/challenges/redis-2025-07-23

## ✨ Features
- **Semantic caching** with Redis 8 Vector (HNSW + cosine)
- **Field-level TTL** for `resp` (answers expire; vectors persist)
- **Per-model / per-route** filtering (`@model:{…} @route:{…}`)
- **OpenAI** embeddings (`text-embedding-3-small`, 1536-dim) and chat (configurable model)
- **Metrics** to a Redis Stream + live **dashboard** (HTMX): hit-rate, p50, p95, sparkline

## Demo UI
Open `GET /` and you’ll see:
- A metrics table — Requests, Hit rate, p50, p95
- A sparkline of the last 20 request latencies  
Both auto-refresh every 2 seconds.

---

## Internals
```
Client → /chat
├─ Embed(prompt) → OpenAI embeddings (1536-d)
├─ KNN search in Redis (HNSW, cosine) with filters: @model, @route
│ └─ If distance ≤ threshold → HGETEX resp (refresh field TTL) → HIT
└─ Else call OpenAI Chat → store:
HSET prompt/model/route/user/created_at/embedding
HSETEX resp with TTL (field-level)
└─ XADD analytics:cache (hit, latency_ms, tokens_saved)
```

**Why field-level TTL?**  
We only TTL the **answer** (`resp`), not the whole hash. Vectors and tags remain in the index to capture future near-duplicates even if a specific answer has expired.

## Environment Variables

| Name | Example | Notes |
|---|---|---|
| `REDIS_URL` | `rediss://default:…@host:port` | TLS required for Redis Cloud |
| `OPENAI_API_KEY` | `sk-...` | For embeddings + chat |
| `EMBED_DIM` | `1536` | For `text-embedding-3-small` |
| `CACHE_DISTANCE_THRESHOLD` | `0.10` | Tune 0.08–0.12 |
| `RESP_TTL_SECS` | `86400` | TTL for `resp` field only |
| `GEN_MODEL` | `gpt-5` | OpenAI chat model |
| `MISS_DELAY_MS` | `1200` | Optional: simulate slow cold path for demos |

Create a `.env` if you like, or set in your shell.

## Redis: Create the index

In `redis-cli` (or RedisInsight Workbench):

```redis
FT.DROPINDEX idx:latency_slayer
FT.CREATE idx:latency_slayer ON HASH PREFIX 1 cache: SCHEMA \
  prompt TEXT \
  model  TAG \
  route  TAG \
  user   TAG \
  embedding VECTOR HNSW 6 TYPE FLOAT32 DIM 1536 DISTANCE_METRIC COSINE

(We store raw f32 bytes; INT8 is possible later with quantization.)
```

## Run locally

cargo run
→ server listening on 0.0.0.0:8080

## Test the cache
```
curl -s -X POST http://localhost:8080/chat \
  -H "content-type: application/json" \
  -d '{"user":"u1","model":"gpt_5","route":"default","messages":[{"role":"user","content":"Explain the features of GPT5."}]}'
# first = cached:false
# second (same prompt) = cached:true with tiny hit_distance
```

Open http://localhost:8080 to see the metrics and sparkline live.


## Contributing
PRs welcome! If you’re adding features (INT8 quantization, alternative providers, multi-tenant keys), please include a short note in the README and keep the sample env vars updated.