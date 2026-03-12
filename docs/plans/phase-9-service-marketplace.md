# Phase 9: Service Marketplace

> External service proxying, dynamic registration, and health checking for the x402 service marketplace.

**Status:** Not started
**Depends on:** Phase 6 (ServiceRegistry + GET /v1/services), Phase 8 (escrow hardening, 384 tests passing)
**Estimated scope:** ~12 new integration tests, ~800 lines of new/modified Rust

---

## Overview

Phase 6 delivered the `ServiceRegistry` (loads `config/services.toml`) and `GET /v1/services` with category/internal filtering (7 integration tests). The registry distinguishes internal services (gateway-hosted endpoints like `/v1/chat/completions`) from external services (third-party x402 endpoints like `https://search.example.com/v1/query`).

Phase 9 makes the marketplace operational:

1. **9.1** — Proxy handler that forwards paid requests to external services
2. **9.2** — Admin API for runtime service registration (in-memory)
3. **9.3** — Background health checker that marks services healthy/unhealthy
4. **9.4** — Integration tests covering all three features

---

## 9.1: External Service Proxy Handler

Route: `POST /v1/services/{service_id}/proxy`

Accepts an arbitrary JSON body, verifies x402 payment, forwards the request to the external service's endpoint, and returns the response.

### Design

- Path parameter `service_id` is looked up in `ServiceRegistry::get()`.
- If the service is `internal: true`, return 400 — internal services have their own routes.
- If no `PAYMENT-SIGNATURE` header, return 402 with cost breakdown. The cost is derived from the service's `pricing_label`. For Phase 9 MVP, external services declare a flat `price_per_request_usdc` field (new field in `services.toml` and `ServiceEntry`). The 5% platform fee is added on top.
- If payment is present, decode and verify via the same `decode_payment_header` + Facilitator flow used in `routes/chat.rs`. Replay protection applies.
- Forward the request body to `service.endpoint` using `state.http_client` (the shared `reqwest::Client` already on `AppState`).
- Timeout: 60 seconds (configurable later).
- If the upstream returns a streaming response (`Transfer-Encoding: chunked` or `Content-Type: text/event-stream`), stream it back to the caller via SSE. Otherwise return the JSON body directly.
- Attach `X-RCR-Request-Id` to the upstream request for traceability.
- Fire-and-forget spend log via `tokio::spawn` (same pattern as `routes/chat.rs`).

### Files to Create/Modify

| File | Action |
|------|--------|
| `crates/gateway/src/routes/services.rs` | Add `service_proxy` handler |
| `crates/gateway/src/services.rs` | Add `price_per_request_usdc: Option<f64>` to `RawServiceEntry` and `ServiceEntry` |
| `config/services.toml` | Add `price_per_request_usdc` to external services |
| `crates/gateway/src/lib.rs` | Register `POST /v1/services/{service_id}/proxy` route |

### 402 Response Shape

```json
{
  "x402_version": 2,
  "resource": {
    "url": "/v1/services/web-search/proxy",
    "method": "POST"
  },
  "accepts": [{
    "scheme": "exact",
    "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
    "amount": "5250",
    "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "pay_to": "<recipient_wallet>"
  }],
  "cost_breakdown": {
    "provider_cost": "0.005000",
    "platform_fee": "0.000250",
    "total": "0.005250",
    "currency": "USDC",
    "fee_percent": 5
  },
  "error": "Payment required"
}
```

### Upstream Error Handling

- 4xx from upstream: return as-is to caller (the caller's request was bad).
- 5xx from upstream: return 502 Bad Gateway with a generic message (do not leak upstream internals).
- Timeout: return 504 Gateway Timeout.
- Connection failure: return 502 with `"service unreachable"`.

---

## 9.2: Service Registration API

Route: `POST /v1/services/register`

Allows runtime registration of new external services. Registrations are in-memory only (lost on restart). Persistent registration is deferred to a future phase with PostgreSQL backing.

### Design

- Protected by `RCR_ADMIN_TOKEN` environment variable. The request must include `Authorization: Bearer <token>`. If the env var is unset, the endpoint returns 404 (not exposed).
- Request body:

```json
{
  "id": "my-custom-api",
  "name": "My Custom API",
  "endpoint": "https://api.example.com/v1/query",
  "category": "data",
  "description": "Custom data API",
  "pricing_label": "$0.01/request",
  "price_per_request_usdc": 0.01
}
```

- Validation rules:
  - `id`: required, unique (not already in registry), `[a-z0-9\-]` only, max 64 chars
  - `endpoint`: required, must start with `https://`
  - `category`: required, non-empty, max 32 chars
  - `name`: required, non-empty, max 128 chars
  - `price_per_request_usdc`: required, must be > 0
- New services are always `internal: false`, `x402_enabled: true`, `chains: ["solana"]`.
- `ServiceRegistry` gains a `register(&mut self, entry: ServiceEntry) -> Result<(), RegistrationError>` method. Since `AppState` holds `ServiceRegistry` behind `Arc`, the registry must be wrapped in a `RwLock` to support mutation.

### Files to Create/Modify

| File | Action |
|------|--------|
| `crates/gateway/src/services.rs` | Add `RwLock` wrapping, `register()` method, `RegistrationError` |
| `crates/gateway/src/routes/services.rs` | Add `register_service` handler |
| `crates/gateway/src/lib.rs` | Update `AppState` to use `RwLock<ServiceRegistry>`, register POST route, conditionally add route based on `RCR_ADMIN_TOKEN` |

### AppState Change

`service_registry: ServiceRegistry` becomes `service_registry: RwLock<ServiceRegistry>`.

All read callers (`list_services`, `service_proxy`, `get` lookups) acquire a read lock. The `register_service` handler acquires a write lock. Contention is negligible (registration is rare).

### Response

- 201 Created with the full `ServiceEntry` JSON on success.
- 409 Conflict if `id` already exists.
- 400 Bad Request with validation errors.
- 401 Unauthorized if token is wrong or missing.
- 404 Not Found if `RCR_ADMIN_TOKEN` is not set (hides the endpoint entirely).

---

## 9.3: Service Health Checking

Background task that periodically probes external services and marks them healthy or unhealthy.

### Design

- Spawned in `main.rs` alongside the claim processor and balance monitor.
- Interval: 60 seconds (configurable via `RCR_SERVICE_HEALTH_INTERVAL_SECS`, default 60).
- For each external service, send a `HEAD` request to `service.endpoint` with a 10-second timeout.
- Mark healthy if response is 2xx or 405 (Method Not Allowed — the service exists but rejects HEAD).
- Mark unhealthy if connection fails, times out, or returns 5xx.
- Health status is stored on `ServiceEntry` as a new field: `healthy: Option<bool>` (`None` = never checked, `Some(true)` = healthy, `Some(false)` = unhealthy).
- `GET /v1/services` includes the `healthy` field in the response.
- Uses the shared `state.http_client` for requests.
- Respects the same `watch` shutdown channel used by the claim processor.

### Files to Create/Modify

| File | Action |
|------|--------|
| `crates/gateway/src/services.rs` | Add `healthy: Option<bool>` to `ServiceEntry`, add `set_health(&self, id: &str, healthy: bool)` method |
| `crates/gateway/src/service_health.rs` | New file: `start_health_checker()` background task |
| `crates/gateway/src/lib.rs` | Declare `pub mod service_health`, spawn task in `main.rs` |
| `crates/gateway/src/main.rs` | Spawn health checker with shutdown signal |
| `crates/gateway/src/routes/services.rs` | Include `healthy` field in JSON response |

### Health State Storage

Since `ServiceRegistry` is now behind `RwLock` (from 9.2), health updates acquire a write lock to set the `healthy` field. The lock is held only for the duration of the field update (not during the HTTP probe).

---

## 9.4: Integration Tests (~12 tests)

All tests use `tower::ServiceExt::oneshot` with `test_app()` — no live server needed.

### Proxy Tests (5)

| # | Test | Assertion |
|---|------|-----------|
| 1 | `test_service_proxy_returns_402_without_payment` | POST to `/v1/services/web-search/proxy` with no payment header returns 402 with cost breakdown |
| 2 | `test_service_proxy_unknown_service_returns_404` | POST to `/v1/services/nonexistent/proxy` returns 404 |
| 3 | `test_service_proxy_internal_service_returns_400` | POST to `/v1/services/llm-gateway/proxy` returns 400 (internal services have dedicated routes) |
| 4 | `test_service_proxy_402_includes_platform_fee` | 402 response `cost_breakdown.fee_percent` is 5 and total includes the 5% markup |
| 5 | `test_service_proxy_402_resource_url_matches_path` | 402 response `resource.url` matches `/v1/services/{id}/proxy` |

### Registration Tests (4)

| # | Test | Assertion |
|---|------|-----------|
| 6 | `test_register_service_requires_admin_token` | POST without `Authorization` header returns 401 |
| 7 | `test_register_service_creates_entry` | POST with valid token and body returns 201; subsequent GET /v1/services includes the new service |
| 8 | `test_register_service_duplicate_id_returns_409` | POST with an ID that already exists returns 409 |
| 9 | `test_register_service_validates_https` | POST with `http://` endpoint returns 400 |

### Health Check Tests (3)

| # | Test | Assertion |
|---|------|-----------|
| 10 | `test_services_response_includes_healthy_field` | GET /v1/services response entries include `healthy` field |
| 11 | `test_services_healthy_null_before_first_check` | Before health checker runs, `healthy` is null |
| 12 | `test_register_service_hidden_without_admin_token` | When `RCR_ADMIN_TOKEN` is not set, POST /v1/services/register returns 404 |

---

## Decision Log

| # | Decision | Rationale |
|---|----------|-----------|
| 89 | Flat `price_per_request_usdc` for external services (not per-token) | External services are opaque; per-token pricing only makes sense for LLMs where we count tokens |
| 90 | `RwLock<ServiceRegistry>` (not `Mutex`) | Reads dominate writes (every proxy request reads, registration is rare); matches Phase E session store pattern (Decision #48) |
| 91 | In-memory registration only (no DB persistence) | YAGNI — persistent registration adds schema, migration, and startup-load complexity. Add when demand exists |
| 92 | HEAD request for health checks, not GET | Avoids triggering metered endpoints; 405 treated as healthy (service exists) |
| 93 | Health status as `Option<bool>`, not enum | Three states (unknown/healthy/unhealthy) map cleanly to `None`/`Some(true)`/`Some(false)`; avoids a new type |
| 94 | Hide register endpoint when `RCR_ADMIN_TOKEN` is unset | Reduces attack surface; 404 reveals nothing |
| 95 | 60s health interval, 10s probe timeout | Matches balance monitor cadence; 10s is generous for a HEAD request |
| 96 | Internal services rejected at proxy (400, not forwarded) | Internal services have dedicated routes with model-specific 402 logic; proxying them would bypass smart routing and per-token pricing |

---

## Build Order

```
9.2 → 9.1 → 9.3 → 9.4
```

**Rationale:** 9.2 introduces the `RwLock` refactor on `ServiceRegistry` and adds the `price_per_request_usdc` field — both are prerequisites for 9.1 (proxy handler) and 9.3 (health checker). 9.1 is the core feature. 9.3 adds background health tracking. 9.4 tests everything.

---

## Implementation Checklist

### 9.2: Service Registration API
- [ ] Add `price_per_request_usdc: Option<f64>` to `RawServiceEntry` and `ServiceEntry`
- [ ] Add `healthy: Option<bool>` to `ServiceEntry` (default `None`)
- [ ] Wrap `ServiceRegistry` internals with `RwLock` (change `services: Vec<ServiceEntry>` to `RwLock<Vec<ServiceEntry>>`)
- [ ] Add `register()` method with validation
- [ ] Add `RegistrationError` variants to `ServiceRegistryError`
- [ ] Update all read sites to acquire read lock (`list_services`, `get`, `all`, `internal`, `external`)
- [ ] Add `register_service` handler in `routes/services.rs`
- [ ] Register POST route in `build_router` (conditional on `RCR_ADMIN_TOKEN`)
- [ ] Update `config/services.toml` with `price_per_request_usdc` on external services
- [ ] Run `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`

### 9.1: External Service Proxy Handler
- [ ] Add `service_proxy` handler in `routes/services.rs`
- [ ] Parse `{service_id}` path parameter
- [ ] Reject internal services with 400
- [ ] Return 402 with cost breakdown when no payment
- [ ] Decode and verify payment (reuse `decode_payment_header` + Facilitator)
- [ ] Forward request to upstream with 60s timeout
- [ ] Handle upstream errors (4xx passthrough, 5xx → 502, timeout → 504)
- [ ] Support streaming responses (SSE passthrough)
- [ ] Fire-and-forget spend log via `tokio::spawn`
- [ ] Register `POST /v1/services/{service_id}/proxy` route in `build_router`
- [ ] Run `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`

### 9.3: Service Health Checking
- [ ] Create `crates/gateway/src/service_health.rs`
- [ ] Implement `start_health_checker()` with shutdown signal
- [ ] HEAD probe with 10s timeout per external service
- [ ] Update `ServiceEntry::healthy` via write lock
- [ ] Spawn in `main.rs` alongside claim processor
- [ ] Add `healthy` field to `GET /v1/services` response
- [ ] Run `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings`

### 9.4: Integration Tests
- [ ] Add 5 proxy tests
- [ ] Add 4 registration tests
- [ ] Add 3 health/registration visibility tests
- [ ] Verify all 384 existing tests still pass
- [ ] Run full `cargo test` — target ~396 tests total
