# Phase 12: Prometheus Monitoring — Implementation Plan

> **Status:** Planned
> **Author:** Kenneth + Claude brainstorming session (2026-03-12)
> **Build Order:** 12.1 (Recorder + Middleware) → 12.2 (Metrics Endpoint) → 12.3 (Domain Instrumentation) → 12.4 (Infrastructure Gauges)

---

## Overview

Add a Prometheus `/metrics` endpoint to the gateway using `metrics` + `metrics-exporter-prometheus`. Exposes 15 metrics across requests, payments, providers, cache, escrow, and infrastructure. Admin-gated behind `RCR_ADMIN_TOKEN`.

### Metrics Catalog

| Metric | Type | Labels | Source |
|--------|------|--------|--------|
| `rcr_requests_total` | counter | method, path, status, model | middleware |
| `rcr_request_duration_seconds` | histogram | method, path | middleware |
| `rcr_active_requests` | gauge | — | middleware |
| `rcr_payments_total` | counter | status (verified/cached/free/none/failed) | chat.rs, proxy.rs |
| `rcr_payment_amount_usdc` | histogram | — | chat.rs, proxy.rs |
| `rcr_replay_rejections_total` | counter | — | chat.rs, proxy.rs |
| `rcr_provider_request_duration_seconds` | histogram | provider | chat.rs |
| `rcr_provider_errors_total` | counter | provider, error_type | chat.rs |
| `rcr_cache_total` | counter | result (hit/miss/skip) | chat.rs |
| `rcr_escrow_claims_total` | counter | result (success/failure) | claim_processor.rs |
| `rcr_escrow_queue_depth` | gauge | — | claim_processor.rs |
| `rcr_fee_payer_balance_sol` | gauge | pubkey | main.rs (balance monitor) |
| `rcr_service_health` | gauge | service_id | service_health.rs |

---

## 12.1: Recorder Initialization + Request Metrics Middleware

### Recorder Setup

**File:** `crates/gateway/src/lib.rs`

In `build_router()`:
1. `PrometheusBuilder::new().build_recorder()` returns `(PrometheusRecorder, PrometheusHandle)`
2. Install recorder globally via `metrics::set_global_recorder(recorder)`
3. Store `PrometheusHandle` in `AppState` for the `/metrics` handler to call `handle.render()`

### Metrics Middleware

**File:** `crates/gateway/src/middleware/metrics.rs`

Tower layer that wraps all routes (except `/metrics` itself):

1. On request entry: increment `rcr_active_requests` gauge, record start time
2. On response: decrement `rcr_active_requests`, record duration to `rcr_request_duration_seconds` histogram, increment `rcr_requests_total` counter with labels (method, path, status code)
3. Skip counting if path is `/metrics` (avoid feedback loop)
4. Model label extracted from request extensions if available, otherwise `"unknown"`

**Layer position in `build_router()`:** After CORS, before tracing — so metrics capture the full request lifecycle including middleware overhead.

### Files

- New: `crates/gateway/src/middleware/metrics.rs`
- Modify: `crates/gateway/src/middleware/mod.rs` — export metrics module
- Modify: `crates/gateway/src/lib.rs` — initialize recorder, add PrometheusHandle to AppState, add metrics layer
- Modify: `crates/gateway/Cargo.toml` — add `metrics`, `metrics-exporter-prometheus`

---

## 12.2: Metrics Endpoint

### Design

**Route:** `GET /metrics`
**Auth:** `Authorization: Bearer <admin-token>` — same pattern as `/v1/escrow/health`

**Handler:**
1. Extract `Authorization` header
2. Validate against `RCR_ADMIN_TOKEN` using `security::constant_time_eq()`
3. If valid: call `state.prometheus_handle.render()` → return with `Content-Type: text/plain; version=0.0.4; charset=utf-8`
4. If invalid/missing: 401

### Files

- New: `crates/gateway/src/routes/metrics.rs`
- Modify: `crates/gateway/src/routes/mod.rs` — export metrics module
- Modify: `crates/gateway/src/lib.rs` — add `GET /metrics` route

---

## 12.3: Domain Instrumentation (Inline)

### Payment Metrics (chat.rs + proxy.rs)

At each payment outcome:
- `counter!("rcr_payments_total", "status" => "verified"|"cached"|"free"|"none"|"failed").increment(1)`
- On successful payment: `histogram!("rcr_payment_amount_usdc").record(amount_f64)`
- On replay rejection: `counter!("rcr_replay_rejections_total").increment(1)`

### Provider Metrics (chat.rs)

Wrap provider call with timing:
- `histogram!("rcr_provider_request_duration_seconds", "provider" => provider_name).record(duration_secs)`
- On provider error: `counter!("rcr_provider_errors_total", "provider" => name, "error_type" => type).increment(1)`

### Cache Metrics (chat.rs)

At cache check result:
- `counter!("rcr_cache_total", "result" => "hit"|"miss"|"skip").increment(1)`

### Escrow Metrics (claim_processor.rs)

At claim outcome:
- `counter!("rcr_escrow_claims_total", "result" => "success"|"failure").increment(1)`
- After fetching pending claims: `gauge!("rcr_escrow_queue_depth").set(queue_len as f64)`

### Files Modified

- `crates/gateway/src/routes/chat.rs` — payment, cache, provider metrics
- `crates/gateway/src/routes/proxy.rs` — payment metrics
- `crates/x402/src/escrow/claim_processor.rs` — escrow claim + queue metrics

---

## 12.4: Infrastructure Gauges

### Fee Payer Balance (main.rs)

In the existing balance monitor loop:
- `gauge!("rcr_fee_payer_balance_sol", "pubkey" => pubkey_str).set(balance_sol)`

### Service Health (service_health.rs)

After updating health status:
- `gauge!("rcr_service_health", "service_id" => id).set(if healthy { 1.0 } else { 0.0 })`

### Files Modified

- `crates/gateway/src/main.rs` — fee payer balance gauge
- `crates/gateway/src/service_health.rs` — service health gauge

---

## Dependency Changes

Add to workspace `Cargo.toml`:
```toml
metrics = "0.24"
metrics-exporter-prometheus = "0.16"
```

Add to `crates/gateway/Cargo.toml`:
```toml
metrics = { workspace = true }
metrics-exporter-prometheus = { workspace = true }
```

Note: Also add `metrics` to `crates/x402/Cargo.toml` for escrow claim metrics in claim_processor.rs.

---

## Tests (12)

1. `/metrics` without auth → 401
2. `/metrics` with valid admin token → 200 with Prometheus text format
3. `/metrics` with invalid token → 401
4. `/metrics` response contains `rcr_requests_total` after a request
5. `/metrics` response contains `rcr_request_duration_seconds` histogram
6. Request to `/v1/chat/completions` increments `rcr_requests_total` with correct labels
7. Payment verified → `rcr_payments_total{status="verified"}` incremented
8. No payment → `rcr_payments_total{status="none"}` incremented
9. Cache hit → `rcr_cache_total{result="hit"}` incremented
10. Provider error → `rcr_provider_errors_total` incremented
11. Service health update → `rcr_service_health` gauge reflects 1/0
12. `/metrics` not counted in its own `rcr_requests_total` (avoids feedback loop)

---

## Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| 96 | `metrics` + `metrics-exporter-prometheus` crate | `prometheus` crate (official client) | Idiomatic Rust, macro-based API, natural Tower/Axum integration |
| 97 | Hybrid: middleware for request metrics, inline for domain metrics | All-middleware, all-inline | Request duration is cross-cutting; payment/cache/escrow data only available in handlers |
| 98 | Admin-gated `/metrics` endpoint | Public, configurable | Financial gateway — metrics expose operational details; consistent with escrow health pattern |
| 99 | `rcr_` prefix on all metric names | No prefix, `rustyclawrouter_` | Short, consistent with `RCR_` env var convention |
| 100 | Global recorder in `build_router()`, handle in AppState | Per-route recorders, lazy_static | Single initialization point; handle needed for `render()` in handler |
| 101 | Exclude `/metrics` from its own request counter | Count it | Avoids feedback loop where scraping inflates request counts |

---

## Non-Functional Requirements

- **Performance:** Metric recording is ~10ns per operation (atomic increments). No allocation on hot path. Middleware adds negligible overhead.
- **Security:** Endpoint gated behind admin token with constant-time comparison. No sensitive data in metric names or labels.
- **Reliability:** If recorder initialization fails, gateway still starts — metrics just won't be available. All metric operations are infallible (fire-and-forget).
- **Backward compatibility:** Fully additive. No existing endpoints or behavior changed.

---

## Implementation Checklist

### 12.1: Recorder + Middleware
- [ ] Add `metrics` and `metrics-exporter-prometheus` to Cargo.toml (workspace + gateway)
- [ ] Add `metrics` to x402 Cargo.toml
- [ ] Create `crates/gateway/src/middleware/metrics.rs`
- [ ] Export in `middleware/mod.rs`
- [ ] Initialize recorder in `build_router()`, store handle in AppState
- [ ] Add metrics layer to router
- [ ] Verify: `cargo check -p gateway`

### 12.2: Metrics Endpoint
- [ ] Create `crates/gateway/src/routes/metrics.rs`
- [ ] Export in `routes/mod.rs`
- [ ] Add `GET /metrics` route in `lib.rs`
- [ ] Verify: `cargo check -p gateway`

### 12.3: Domain Instrumentation
- [ ] Add payment metrics to `routes/chat.rs`
- [ ] Add payment metrics to `routes/proxy.rs`
- [ ] Add provider metrics to `routes/chat.rs`
- [ ] Add cache metrics to `routes/chat.rs`
- [ ] Add escrow metrics to `claim_processor.rs`
- [ ] Verify: `cargo check -p gateway && cargo check -p x402`

### 12.4: Infrastructure Gauges
- [ ] Add fee payer balance gauge to `main.rs`
- [ ] Add service health gauge to `service_health.rs`
- [ ] Verify: `cargo check -p gateway`

### Tests
- [ ] Write 12 tests
- [ ] Verify: `cargo test -p gateway`
- [ ] Verify: `cargo clippy --all-targets --all-features -- -D warnings`
- [ ] Verify: `cargo fmt --all -- --check`

### Final
- [ ] Full test suite passes: `cargo test`
- [ ] Update HANDOFF.md with Phase 12 completion
