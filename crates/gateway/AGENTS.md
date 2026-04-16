<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# gateway

## Purpose
The only binary in the workspace. An Axum HTTP server that exposes OpenAI-compatible chat endpoints, enforces x402 USDC-SPL payments, proxies to upstream LLM providers, tracks usage in PostgreSQL, caches in Redis, emits Prometheus metrics, and hosts the A2A protocol adapter and enterprise org/team/API-key system. Binary name: `solvela-gateway`. Listens on `:8402` by default.

## Key Files
| File | Description |
|------|-------------|
| `Cargo.toml` | Crate manifest — `lib` target + `solvela-gateway` bin target |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | All application code (see `src/AGENTS.md`) |
| `tests/` | Integration tests using `tower::ServiceExt::oneshot` (see `tests/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Route handlers return `Result<impl IntoResponse, GatewayError>`.
- Share state via `State(Arc<AppState>)` — fields on `AppState` are public so handlers can reach them.
- Request flow: parse → resolve model → prompt-guard → x402 check (middleware extracts payment, route enforces 402) → provider proxy → cache → usage log → return.
- All DB writes are fire-and-forget: `tokio::spawn(async move { … })` — never `.await` database writes on the request hot path.
- Middleware is layered bottom-up in `build_router` — innermost layer executes last.

### Testing Requirements
```bash
cargo test -p gateway                      # unit + integration
cargo test -p gateway --test integration   # integration only
cargo test -p gateway --lib                # unit only
cargo test -p gateway -- --nocapture       # show tracing output
```

### Common Patterns
- Custom `Debug` redaction on any struct that holds secrets (`config.rs`).
- Structured logging: `tracing::info!(wallet = %addr, model = %model, "...")`.
- Extractors: `Extension<Option<PaymentInfo>>`, `RequireOrg`, `RequireOrgAdmin`.
- Provider adapters translate OpenAI ↔ native format — gateway always speaks OpenAI-compat externally.

## Dependencies

### Internal
- `solvela-protocol` — wire-format types
- `x402` (with `postgres` feature) — payment verification, escrow
- `solvela-router` — scorer + model registry

### External
- `axum`, `tower`, `tower-http`, `tokio`, `reqwest`, `sqlx`, `redis`, `metrics-exporter-prometheus`, `lru`, `uuid`, `chrono`, `base64`, `bs58`.

<!-- MANUAL: -->
