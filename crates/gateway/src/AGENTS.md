<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
All gateway source. `lib.rs` wires middleware, routes, and shared state into an `axum::Router`; `main.rs` constructs the runtime and launches the server. Other top-level modules provide cross-cutting services (config, cache, usage logging, provider registry, audit log, org hierarchy).

## Key Files
| File | Description |
|------|-------------|
| `lib.rs` | Library entry — declares modules, builds the top-level `axum::Router`, wires tower layers |
| `main.rs` | Binary entry — loads config, connects PG/Redis, applies all migration files via `sqlx::migrate!` on startup (fails fast if any migration errors), starts server |
| `config.rs` | `AppConfig` + custom `Debug` redaction; env prefix `SOLVELA_` (legacy `RCR_` accepted) |
| `error.rs` | `GatewayError` + `IntoResponse` — converts internal errors into HTTP responses |
| `cache.rs` | Redis-backed response cache (degrades gracefully if Redis is absent) |
| `usage.rs` | Fire-and-forget usage logger + `wallet_budgets` queries |
| `audit.rs` | Fire-and-forget audit-log writer (org-scoped events) |
| `security.rs` | Prompt-guard types + PII/injection detection helpers |
| `service_health.rs` | Background health checks for upstream providers |
| `services.rs` | `ServiceRegistry` — loads `config/services.toml`, exposes marketplace |
| `session.rs` | Per-request session IDs, shared state helpers |
| `balance_monitor.rs` | Watches fee-payer / recipient balances and alerts below thresholds |
| `payment_util.rs` | Shared helpers for computing cost + 402 payloads |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `a2a/` | A2A protocol (JSON-RPC) adapter — agent card, handler, task store (see `a2a/AGENTS.md`) |
| `middleware/` | Tower/Axum middleware layers (see `middleware/AGENTS.md`) |
| `orgs/` | Enterprise org + team data model + queries (see `orgs/AGENTS.md`) |
| `providers/` | LLM provider adapters (OpenAI, Anthropic, Google, xAI, DeepSeek) (see `providers/AGENTS.md`) |
| `routes/` | HTTP route handlers (see `routes/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Declare new modules in `lib.rs` with `pub mod foo;` before referencing them elsewhere.
- Secrets never touch `serde_json::to_string(&config)` — fields holding keys implement custom `Debug`.
- A2A is a **protocol adapter**, not new payment logic — it translates JSON-RPC to the existing x402 + chat pipeline.
- Prefer small modules (200–400 lines); split when approaching 800.

### Testing Requirements
```bash
cargo test -p gateway
```
Integration tests in `../tests/` drive the router via `tower::ServiceExt::oneshot(test_app())` — no live server required.

### Common Patterns
- `AppState` fields are public and wrapped in `Arc<…>` / `Arc<RwLock<…>>` as needed.
- Background tasks spawned with `tokio::spawn`; never await on DB writes during a request.
- Config values read with `#[serde(default)]` + `Option<T>` so missing values degrade gracefully.

## Dependencies

### Internal
- `crate::config`, `crate::cache`, `crate::usage`, `crate::error` are used by nearly every module.
- `solvela-protocol` (re-exports) for wire-format types.
- `x402` for payment verification.
- `solvela-router` for request scoring + model registry.

### External
- `axum`, `tower`, `tower-http`, `tokio`, `tracing`, `sqlx`, `redis`, `reqwest`, `metrics`.

<!-- MANUAL: -->
