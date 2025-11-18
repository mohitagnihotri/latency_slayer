use super::AppState;
use crate::redis_store::{read_last_latencies, read_metrics};
use crate::types::MetricsOut;
use axum::{extract::State, http::StatusCode, response::Response, Json};
use std::sync::Arc;

pub async fn metrics_endpoint(
    State(state): State<Arc<AppState>>,
) -> Result<Json<MetricsOut>, StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let metrics = read_metrics(&mut conn, 200).await.unwrap_or_default();
    Ok(Json(metrics))
}

pub async fn metrics_fragment(
    State(state): State<Arc<AppState>>,
) -> Result<Response<String>, StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

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

    Ok(Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html)
        .unwrap())
}

pub async fn metrics_sparkline(
    State(state): State<Arc<AppState>>,
) -> Result<Response<String>, StatusCode> {
    let mut conn = state
        .redis
        .get_multiplexed_async_connection()
        .await
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

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
            w = w,
            h = h,
            pts = pts.trim(),
            cx = last_xy.0,
            cy = last_xy.1
        )
    } else {
        "<div id=\"spark\"><em class=\"muted\">No data yet</em></div>".to_string()
    };

    Ok(Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html)
        .unwrap())
}
