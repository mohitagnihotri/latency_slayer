use axum::response::Response;

pub async fn index_html() -> Response<String> {
    let html = r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>Latency Slayer Dashboard</title>
  <script src="https://unpkg.com/htmx.org@1.9.12" integrity="sha384-IQOtF1RZb5/Cd8tQz6qqn9qJbTtF5Vle0VjyFoP3n3oD8uH3Qf6P2kF0B1EM6w7U" crossorigin="anonymous"></script>
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
  <h1>Latency Slayer <span class="muted">(semantic cache)</span></h1>
  <div class="card">
    <div id="kpis" class="kpi" hx-get="/metrics/fragment" hx-trigger="load, every 2s" hx-swap="innerHTML">
      <div><div class="muted">Requests</div><div>–</div></div>
      <div><div class="muted">Hit rate</div><div>–</div></div>
      <div><div class="muted">p50 (ms)</div><div>–</div></div>
      <div><div class="muted">p95 (ms)</div><div>–</div></div>
    </div>
    <p class="muted" style="margin-top:12px">Auto-refreshes every 2s. Open DevTools → Network to see the HTMX calls.</p>
  </div>
</body>
</html>"#;
    Response::builder()
        .header("content-type", "text/html; charset=utf-8")
        .body(html.to_string())
        .unwrap()
}
