<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# routes

## Purpose
All HTTP route handlers. Every handler returns `Result<impl IntoResponse, GatewayError>` and reads shared state via `State(Arc<AppState>)`. This is also where `402 Payment Required` responses are produced — the x402 middleware only extracts payment info; routes enforce the 402.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root — re-exports each route module |
| `health.rs` | `GET /health` + `GET /ready` — liveness/readiness probes |
| `models.rs` | `GET /v1/models` + `GET /models` — OpenAI-compat model listing from the router's registry |
| `supported.rs` | `GET /supported` — richer catalogue (per-profile, capabilities, prices) |
| `pricing.rs` | `GET /pricing` — per-model pricing; used by dashboards |
| `services.rs` | `GET /services` + related — the x402 service marketplace from `config/services.toml` |
| `escrow.rs` | `POST /escrow/*` — escrow deposit/claim/refund endpoints |
| `nonce.rs` | `POST /nonce` — durable-nonce account allocation for offline signing |
| `proxy.rs` | Generic provider proxy (non-chat) — e.g., embeddings |
| `images.rs` | `POST /v1/images/generations` — image generation route |
| `stats.rs` | Per-wallet / public stats endpoints |
| `admin_stats.rs` | Admin-only stats (gated by API-key role) |
| `metrics.rs` | `GET /metrics` — Prometheus text format |
| `debug_headers.rs` | Debug endpoint echoing selected headers — used in dev only |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `chat/` | Chat-completions pipeline split by concern: cost, payment, provider, response (see `chat/AGENTS.md`) |
| `orgs/` | Enterprise org/team/API-key CRUD + analytics + budget (see `orgs/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Routes enforce 402; middleware extracts. Do not call `verify_payment` here — use the `Option<PaymentInfo>` that the middleware already attached to request extensions.
- When adding a new public endpoint, register it in `mod.rs` and add an entry in `build_router` in `crate::lib`.
- OpenAI-compatibility is load-bearing: use the same field names and error shapes as OpenAI for endpoints prefixed `/v1/`.
- Add CORS headers at the router layer (already configured), not per-route.

### Testing Requirements
```bash
cargo test -p gateway --test integration
```
Integration tests use `tower::ServiceExt::oneshot(test_app())` — no live server required.

### Common Patterns
- `Query<T>` extractors with `#[serde(default)]` fields on optional params.
- `Json<T>` for request bodies; return `Json<T>` for JSON responses.
- Stream responses with `axum::response::sse::Sse` for SSE.

## Dependencies

### Internal
- `crate::error::GatewayError`, `crate::middleware`, `crate::providers`, `crate::services`, `crate::orgs`, `crate::cache`, `crate::usage`, `solvela-protocol`, `x402`, `solvela-router`.

### External
- `axum`, `tower`, `serde`, `serde_json`, `tokio`, `futures`, `tracing`.

<!-- MANUAL: -->
