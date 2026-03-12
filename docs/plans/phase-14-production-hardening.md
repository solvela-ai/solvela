# Phase 14: Production Hardening — Implementation Plan

> **Status:** Planned
> **Author:** Kenneth + Claude brainstorming session (2026-03-12)
> **Build Order:** 14.1 (Safety Layers) → 14.2 (Lifecycle) → 14.3 (Observability) → 14.4 (Performance) → 14.5 (Validation & Resilience)

---

## Overview

Production hardening based on comprehensive audit of shutdown lifecycle, request safety, infrastructure, and error handling. All changes are internal — no new endpoints, no API breaking changes, no architectural changes.

### Audit Findings Addressed

| Priority | Issue | Fix |
|----------|-------|-----|
| HIGH | No CatchPanicLayer — panics kill request silently | 14.1 |
| HIGH | No request timeout — requests hang indefinitely | 14.1 |
| HIGH | No connection concurrency limit — unbounded tasks | 14.1 |
| HIGH | Provider clients created per-adapter, not shared | 14.4 |
| HIGH | Balance monitor + rate limiter cleanup lack shutdown signal | 14.2 |
| MEDIUM | PostgreSQL pool at default 5 connections | 14.4 |
| MEDIUM | Health endpoint is liveness-only, no dependency checks | 14.3 |
| MEDIUM | No JSON structured logging for production | 14.3 |
| MEDIUM | No max message count validation in chat requests | 14.5 |
| MEDIUM | No provider call retries for transient failures | 14.5 |

---

## 14.1: Safety Layers

### CatchPanicLayer

**Position:** Outermost layer in `build_router()` (wraps everything including metrics).

Catches handler panics and returns a 500 JSON response instead of dropping the TCP connection. Uses `tower_http::catch_panic::CatchPanicLayer`.

Custom response handler to return consistent JSON:
```rust
fn handle_panic(_err: Box<dyn std::any::Any + Send + 'static>) -> Response {
    let body = serde_json::json!({
        "error": {
            "type": "internal_error",
            "message": "Internal server error"
        }
    });
    (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
}
```

### TimeoutLayer

**Position:** After CatchPanicLayer, before metrics layer.

Global 120-second request timeout. Configurable via `RCR_REQUEST_TIMEOUT_SECS` env var (default 120). Returns 408 Request Timeout on expiry.

Uses `tower_http::timeout::TimeoutLayer`.

### ConcurrencyLimitLayer

**Position:** After TimeoutLayer, before CORS.

Max 256 concurrent in-flight requests. Configurable via `RCR_MAX_CONCURRENT_REQUESTS` env var (default 256). Rejects excess with 503 Service Unavailable.

Uses `tower::limit::ConcurrencyLimitLayer`.

### Files

- Modify: `crates/gateway/Cargo.toml` — enable `catch-panic` feature on `tower-http`
- Modify: `crates/gateway/src/lib.rs` — add CatchPanicLayer, TimeoutLayer, ConcurrencyLimitLayer to router

---

## 14.2: Lifecycle — Graceful Shutdown for All Tasks

### Balance Monitor

Add `shutdown_rx: tokio::sync::watch::Receiver<bool>` parameter to `BalanceMonitor::spawn()`. Wrap the tick loop in `tokio::select!` with the shutdown signal:

```rust
tokio::select! {
    _ = ticker.tick() => { /* existing check_balances + emit_alerts */ }
    _ = shutdown_rx.changed() => {
        tracing::info!("balance monitor shutting down gracefully");
        break;
    }
}
```

### Rate Limiter Cleanup

Extract the inline `tokio::spawn` loop in `main.rs` into a function that accepts `shutdown_rx`. Same `tokio::select!` pattern.

### Shutdown Sequence (Complete)

```
SIGTERM/Ctrl+C
  → axum::serve graceful shutdown (drains in-flight)
  → shutdown_tx.send(true)
    → claim processor exits
    → health checker exits
    → balance monitor exits (new)
    → rate limiter cleanup exits (new)
  → AppState dropped → DB/Redis closed
```

### Files

- Modify: `crates/gateway/src/balance_monitor.rs` — add shutdown_rx to spawn()
- Modify: `crates/gateway/src/main.rs` — pass shutdown_rx to balance monitor and rate limiter cleanup

---

## 14.3: Observability

### Readiness Health Check

Expand `GET /health` to return dependency status. Keep unauthenticated (load balancers need it).

Response shape:
```json
{
  "status": "ok",
  "version": "0.1.0",
  "checks": {
    "database": "connected",
    "redis": "connected",
    "providers": ["anthropic", "openai"],
    "solana_rpc": "reachable"
  }
}
```

Status logic:
- `"ok"` — at least one provider configured, DB and Redis connected (or not configured)
- `"degraded"` — at least one provider but DB or Redis unavailable
- `"error"` — zero providers configured

HTTP status always 200 (Fly.io health checks need 2xx).

### JSON Structured Logging

Add `RCR_LOG_FORMAT` env var. Default `"text"` (current behavior). Set to `"json"` for production log aggregation.

```rust
let subscriber = tracing_subscriber::fmt()
    .with_env_filter(filter);

match std::env::var("RCR_LOG_FORMAT").as_deref() {
    Ok("json") => subscriber.json().init(),
    _ => subscriber.init(),
};
```

### Files

- Modify: `crates/gateway/src/routes/health.rs` — expand with dependency checks
- Modify: `crates/gateway/src/main.rs` — conditional JSON logging, pass dependency state
- Modify: `crates/gateway/src/lib.rs` — health handler needs access to AppState

---

## 14.4: Performance

### PostgreSQL Pool Configuration

Replace `PgPool::connect(&url)` with:
```rust
sqlx::postgres::PgPoolOptions::new()
    .max_connections(max_conn)
    .acquire_timeout(Duration::from_secs(5))
    .connect(&url)
    .await
```

Configurable via `RCR_DB_MAX_CONNECTIONS` env var (default 20).

### Shared HTTP Client for Providers

Refactor all provider adapters to accept a `reqwest::Client` from AppState:

```rust
pub struct OpenAIProvider {
    client: reqwest::Client,  // shared, not self-created
    api_key: String,
}

impl OpenAIProvider {
    pub fn new(client: reqwest::Client, api_key: String) -> Self {
        Self { client, api_key }
    }
}
```

Provider-specific timeout: 90 seconds per LLM request, set on the request builder:
```rust
self.client
    .post(url)
    .timeout(Duration::from_secs(90))
    .json(&req)
    .send()
    .await
```

### Files

- Modify: `crates/gateway/src/main.rs` — PgPoolOptions, pass shared client to providers
- Modify: `crates/gateway/src/providers/openai.rs` — accept shared client
- Modify: `crates/gateway/src/providers/anthropic.rs` — accept shared client
- Modify: `crates/gateway/src/providers/google.rs` — accept shared client
- Modify: `crates/gateway/src/providers/xai.rs` — accept shared client
- Modify: `crates/gateway/src/providers/deepseek.rs` — accept shared client
- Modify: `crates/gateway/src/lib.rs` — pass shared client when constructing providers

---

## 14.5: Validation & Resilience

### Max Message Count

Add `const MAX_MESSAGES: usize = 256` in chat handler. Validate before processing:

```rust
if request.messages.len() > MAX_MESSAGES {
    return Err(GatewayError::BadRequest(
        format!("too many messages: {} exceeds maximum of {}", request.messages.len(), MAX_MESSAGES)
    ));
}
```

### Provider Retry with Backoff

Add `retry_with_backoff` helper in `crates/gateway/src/providers/mod.rs`:

```rust
pub async fn retry_with_backoff<F, Fut, T, E>(
    max_retries: u32,
    f: F,
) -> Result<T, E>
where
    F: Fn() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut last_err = None;
    for attempt in 0..=max_retries {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                if attempt < max_retries && is_transient(&e) {
                    let delay = Duration::from_secs(1 << attempt); // 1s, 2s
                    tokio::time::sleep(delay).await;
                    last_err = Some(e);
                } else {
                    return Err(e);
                }
            }
        }
    }
    Err(last_err.unwrap())
}
```

Transient = timeout or 5xx. NOT retried: 4xx (auth, rate limit, bad request).

### Files

- Modify: `crates/gateway/src/routes/chat.rs` — max message count validation
- Modify: `crates/gateway/src/providers/mod.rs` — retry_with_backoff helper
- Modify: `crates/gateway/src/providers/openai.rs` — wrap send in retry
- Modify: `crates/gateway/src/providers/anthropic.rs` — wrap send in retry
- Modify: `crates/gateway/src/providers/google.rs` — wrap send in retry
- Modify: `crates/gateway/src/providers/xai.rs` — wrap send in retry
- Modify: `crates/gateway/src/providers/deepseek.rs` — wrap send in retry

---

## Tests (14)

1. Request exceeding timeout → 408 Request Timeout
2. Handler panic → 500 JSON response (not connection drop)
3. Concurrent requests beyond limit → 503 Service Unavailable
4. Balance monitor stops on shutdown signal
5. Rate limiter cleanup stops on shutdown signal
6. `GET /health` returns provider list and dependency status
7. `GET /health` returns `"degraded"` when no DB configured
8. `GET /health` returns `"error"` when no providers configured
9. Chat request with >256 messages → 400 Bad Request
10. Chat request with exactly 256 messages → accepted
11. Provider transient failure (5xx) retried successfully
12. Provider auth failure (401) NOT retried
13. PostgreSQL pool respects max_connections setting
14. JSON log format enabled via `RCR_LOG_FORMAT=json`

---

## Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| 107 | CatchPanicLayer as outermost middleware | Per-handler catch, no protection | Must wrap everything; panics should never kill connections |
| 108 | 120s global request timeout (configurable) | 30s, 60s, no timeout | Streaming LLM responses need 60-90s; 120s gives headroom |
| 109 | 256 concurrent request limit (configurable) | 512, 1024, unlimited | Conservative for 512MB Fly.io VM; configurable for scaling |
| 110 | Watch channel shutdown for all background tasks | tokio::select on JoinHandle, no cleanup | Consistent with existing claim processor/health checker pattern |
| 111 | Readiness health check with dependency status | Separate /ready endpoint, keep liveness only | Single endpoint simpler; status field distinguishes ok/degraded/error |
| 112 | JSON logging via RCR_LOG_FORMAT env var | Always JSON, always text | Operators choose; text for dev, JSON for production log aggregation |
| 113 | PgPool max_connections=20 (configurable) | Default 5, fixed 50 | 20 reasonable for single-instance; env var for tuning |
| 114 | Shared reqwest::Client for all providers | Per-provider clients, connection pool per provider | Reuses TCP connections; single config point |
| 115 | 90s per-request timeout for LLM calls | 10s (current global), 60s, unlimited | Streaming responses to large prompts need 60-90s |
| 116 | Max 256 messages per chat request | 128, 512, no limit | Balances usability with DoS prevention |
| 117 | 2 retries with exponential backoff for transient provider failures | No retry, 5 retries, circuit breaker only | Quick recovery from blips without excess latency |

---

## Non-Functional Requirements

- **Performance:** Safety layers add <1us overhead (atomic operations). Provider retries add latency only on failure (1-3s backoff). Shared HTTP client reduces TCP handshakes.
- **Security:** CatchPanicLayer prevents information leakage via dropped connections. Concurrency limit prevents resource exhaustion. No new attack surface.
- **Reliability:** All background tasks shut down cleanly. No orphaned connections. Health check enables accurate load balancer decisions.
- **Backward compatibility:** All changes internal. No API changes. Existing clients unaffected.

---

## Implementation Checklist

### 14.1: Safety Layers
- [ ] Enable `catch-panic` feature on `tower-http` in Cargo.toml
- [ ] Add CatchPanicLayer with custom JSON response handler
- [ ] Add TimeoutLayer (120s default, configurable)
- [ ] Add ConcurrencyLimitLayer (256 default, configurable)
- [ ] Verify layer ordering: CatchPanic → Timeout → Concurrency → CORS → Metrics → ...
- [ ] Verify: `cargo check -p gateway`

### 14.2: Lifecycle
- [ ] Add shutdown_rx to BalanceMonitor::spawn()
- [ ] Add shutdown_rx to rate limiter cleanup loop
- [ ] Pass shutdown_rx from main.rs to both tasks
- [ ] Verify: `cargo check -p gateway`

### 14.3: Observability
- [ ] Expand health handler with dependency checks
- [ ] Health handler needs AppState access (may need to change route registration)
- [ ] Add JSON logging via RCR_LOG_FORMAT env var
- [ ] Verify: `cargo check -p gateway`

### 14.4: Performance
- [ ] Replace PgPool::connect with PgPoolOptions
- [ ] Add RCR_DB_MAX_CONNECTIONS env var (default 20)
- [ ] Refactor OpenAI provider to accept shared client
- [ ] Refactor Anthropic provider to accept shared client
- [ ] Refactor Google provider to accept shared client
- [ ] Refactor xAI provider to accept shared client
- [ ] Refactor DeepSeek provider to accept shared client
- [ ] Set 90s per-request timeout on LLM calls
- [ ] Verify: `cargo check -p gateway`

### 14.5: Validation & Resilience
- [ ] Add MAX_MESSAGES constant (256)
- [ ] Add message count validation in chat handler
- [ ] Implement retry_with_backoff helper
- [ ] Wrap all provider send() calls in retry
- [ ] Verify: `cargo check -p gateway`

### Tests
- [ ] Write 14 tests
- [ ] Verify: `cargo test -p gateway`
- [ ] Verify: `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] Verify: `cargo fmt --all -- --check`

### Final
- [ ] Full test suite passes: `cargo test`
- [ ] Update HANDOFF.md with Phase 14 completion
