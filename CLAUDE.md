# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This Is

Solvela is a Solana-native AI agent payment gateway built in Rust (Axum). AI agents pay for LLM API calls with USDC-SPL on Solana via the x402 protocol. No API keys, no accounts, just wallets.

Read `.claude/plan/solvela.md` for the full implementation plan. See `HANDOFF.md` for current project status and what's been completed. See `CHANGELOG.md` for chronological history.

## Build & Test Commands

```bash
# Build
cargo build                       # full workspace
cargo check                       # faster — prefer for iteration
cargo check -p gateway            # single crate
cargo build --release             # release build

# Test — run `cargo test` for current counts (don't trust hardcoded numbers)
cargo test                        # all workspace tests
cargo test -p gateway             # gateway (unit + integration)
cargo test -p x402                # x402 protocol
cargo test -p router              # smart router
cargo test -p solvela-protocol    # wire-format types
cargo test -p solvela-cli         # CLI

# Single test
cargo test -p gateway test_health_endpoint -- --exact

# Pattern match
cargo test scorer                 # all tests matching "scorer"

# Scoped test runs
cargo test -p gateway --test integration  # integration tests only
cargo test -p gateway --lib               # unit tests only
cargo test -p x402 -- --nocapture         # show stdout/tracing output

# Escrow program (standalone — NOT in workspace)
cargo test --manifest-path programs/escrow/Cargo.toml
# If OpenSSL issues on Linux:
OPENSSL_NO_PKG_CONFIG=1 OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test --manifest-path programs/escrow/Cargo.toml

# Lint (must pass before committing)
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --all -- --check        # CI mode (check only)

# Dashboard tests
npm --prefix dashboard test

# Go SDK tests
go test ./... -v

# Local dev stack
docker compose up -d              # PostgreSQL + Redis
cp .env.example .env              # fill in at least one provider API key
RUST_LOG=info cargo run -p gateway  # listens on :8402
```

Migrations in `migrations/` are applied automatically by `docker compose up` and on gateway startup via `run_migrations()` (idempotent).

## Architecture

### Workspace Crates (`crates/`)

- **gateway** — The only binary. Axum HTTP server with routes, middleware, provider proxies, usage tracking, caching, Prometheus metrics, service marketplace, and `ServiceRegistry`. Key modules:
  - `routes/chat/` — Chat completions (mod.rs, cost.rs, payment.rs, provider.rs, response.rs)
  - `a2a/` — A2A protocol adapter (types.rs, agent_card.rs, jsonrpc.rs, handler.rs, task_store.rs)
  - `orgs/` — Enterprise org/team hierarchy (models.rs, queries.rs)
  - `routes/orgs/` — Org API endpoints (crud.rs, teams.rs, api_keys.rs, audit.rs, budget.rs, analytics.rs)
  - `audit.rs` — Fire-and-forget audit log writer
  - `middleware/api_key.rs` — `OrgContext`, `RequireOrg`/`RequireOrgAdmin` extractors
  - Binary name: `solvela-gateway`
- **x402** — Pure protocol library (no Axum dependency). Solana verification, escrow integration, fee payer pool, nonce pool. Chain-agnostic `PaymentVerifier` trait for future EVM support.
- **router** — 15-dimension rule-based request scorer, routing profiles (eco/auto/premium/free), and model registry (loads `config/models.toml`).
- **protocol** (`solvela-protocol`) — Shared wire-format types. Payment protocol types, OpenAI-compatible chat types, model info, constants. Zero workspace dependencies.
- **cli** (`solvela-cli`) — `solvela` CLI binary (clap derive): wallet, chat, models, health, stats, doctor commands.

### Standalone Anchor Program (`programs/escrow/`)

Trustless USDC-SPL escrow with deposit/claim/refund instructions. **NOT a workspace member** to avoid dep version conflicts. PDA seeds: `[b"escrow", agent.key().as_ref(), &service_id]`.

### Configuration (`config/`)

- `models.toml` — Model registry with per-token pricing (5 providers, 26+ models)
- `default.toml` — Server host/port, Solana RPC URL, monitor thresholds
- `services.toml` — x402 service marketplace registry

### SDKs (`sdks/`)

Python, TypeScript, Go, and MCP server SDKs. Each has its own test suite.

### Dashboard (`dashboard/`)

Next.js 16 + Tailwind + Recharts. Pages: Overview, Usage, Models, Wallet, Settings.

## Key Architectural Rules

1. **`gateway` is the only binary** — all other crates are libraries (`lib.rs` only)
2. **`x402` has no Axum dependency** — pure protocol library, no HTTP framework coupling
3. **`PaymentVerifier` trait is chain-agnostic** — designed for future EVM/Base support
4. **Provider adapters translate OpenAI <-> native format** — gateway always speaks OpenAI format
5. **5% platform fee on all requests** — always include `cost_breakdown` in payment info
6. **Solana-first** — Base/EVM is a future feature; do not implement EVM paths now
7. **Never store private keys** — wallet keys stay client-side; only signed txs reach the gateway
8. **Payment middleware extracts, routes enforce** — `middleware/x402.rs` never returns 402; `routes/chat.rs` does
9. **All DB writes are fire-and-forget** — `tokio::spawn`; never `.await` on the hot path
10. **Integration tests need no live server** — use `tower::ServiceExt::oneshot` with `test_app()`
11. **Escrow program is NOT a workspace member** — avoids dep version conflicts; build separately
12. **Both PostgreSQL and Redis are optional** — gateway degrades gracefully when either is absent
13. **API key auth uses extractor pattern** — `RequireOrg` and `RequireOrgAdmin` are Axum extractors that populate `OrgContext`; org-scoped routes extract `OrgContext` from request extensions, never from query params or body
14. **A2A is a protocol adapter, not new payment logic** — translates A2A JSON-RPC to existing x402 + chat pipeline. No fiat, no AP2 mandates, no card processing.
15. **Migration failure is fatal when a DB is configured** — if `DATABASE_URL` is set and `sqlx::migrate!` fails during startup, `run_migrations()` propagates the error and `main()` exits non-zero. Do not reintroduce a `warn!` + continue pattern: serving traffic against a broken schema produces silent query errors on every org/audit/budget path. Graceful degradation only applies when `DATABASE_URL` is unset entirely.

## Request Flow (POST /v1/chat/completions)

1. Parse request, resolve model (aliases like "sonnet" / profiles like "auto" / direct IDs)
2. Prompt guard checks (injection, jailbreak, PII detection)
3. If no `PAYMENT-SIGNATURE` header -> return 402 with cost breakdown + accepted payment schemes (exact + escrow if configured)
4. If header present -> decode (base64 or raw JSON) -> replay protection (Redis) -> verify via Facilitator -> proxy to LLM provider
5. Cache response (Redis), log spend (PostgreSQL), fire escrow claim if applicable
6. Return JSON or SSE stream; fall through to stub if no provider configured

## A2A Request Flow (POST /a2a, method: message/send)

1. Agent discovers Solvela via `GET /.well-known/agent.json` (AgentCard with AP2 + x402 extensions)
2. Agent sends `message/send` JSON-RPC with text prompt → gateway computes cost → returns Task (`input-required`) with `x402.payment.required` metadata
3. Agent signs Solana USDC-SPL transaction → sends `message/send` with `taskId` + `x402.payment.payload` metadata
4. Gateway verifies payment (reuses facilitator), proxies to LLM provider → returns Task (`completed`) with artifacts + receipt

## Smart Router

The scorer in `crates/router/src/scorer.rs` classifies requests across 15 weighted dimensions (code presence, reasoning markers, technical terms, etc.) into tiers: Simple / Medium / Complex / Reasoning. Each routing profile (eco/auto/premium/free) maps tiers to specific models. Scoring is pure rule-based, <1us, zero external calls.

## Environment Variables

Provider API keys: `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`, `GOOGLE_API_KEY`, `XAI_API_KEY`, `DEEPSEEK_API_KEY`. Gateway config uses `SOLVELA_` prefix (legacy `RCR_` accepted with deprecation warning). Solana keys: `SOLVELA_SOLANA_RPC_URL`, `SOLVELA_SOLANA_RECIPIENT_WALLET`, `SOLVELA_SOLANA_FEE_PAYER_KEY`. Optional: `DATABASE_URL`, `REDIS_URL`. See `.env.example` for full list.

## Code Conventions

### Rust Conventions

- **Edition**: Rust 2021 (`resolver = "2"` workspace) — migration to 2024 planned
- **Async runtime**: Tokio — use `#[tokio::main]` and `#[tokio::test]`
- **Web framework**: Axum 0.8 with Tower middleware layers
- **Error handling**:
  - `thiserror` for all library/crate-level error enums
  - `anyhow` only in binary entry points (`main.rs`) and tests
  - Never use `unwrap()` or `expect()` in library code — propagate with `?`
- **Naming**:
  - Types/traits: `PascalCase` — `PaymentRequired`, `ServiceRegistry`
  - Functions/methods: `snake_case` — `verify_payment`, `list_services`
  - Constants: `SCREAMING_SNAKE_CASE` — `USDC_MINT`, `PLATFORM_FEE_PERCENT`
  - Modules/files: `snake_case` — `rate_limit.rs`, `services.rs`
  - Crate names: `kebab-case` in Cargo.toml, `snake_case` in `use` statements

### Struct & Enum Patterns

- Derive order: `Debug, Clone, Serialize, Deserialize` — Serde always last
- Error enums: `#[derive(Debug, thiserror::Error)]` with `#[error("...")]` on every variant
- Async trait methods: annotate with `#[async_trait::async_trait]`
- Thread-safe traits: bound with `Send + Sync` (required for Axum `State`)
- Config structs: `#[derive(Debug, Clone, Deserialize)]` — no `Serialize` unless needed
- Use `#[serde(default)]` on optional config fields with `Option<T>`

### Import Ordering

Separate groups with a blank line; `rustfmt` enforces this:

```rust
use std::sync::Arc;                          // 1. Standard library

use axum::{Router, routing::{get, post}};   // 2. External crates (alphabetical)
use serde::{Deserialize, Serialize};
use tracing::info;

use solvela_protocol::ChatRequest;           // 3. Workspace crates

use crate::config::AppConfig;               // 4. Crate-internal modules
```

### Axum Patterns

- Route handlers return `Result<impl IntoResponse, GatewayError>`
- Share state with `State(Arc<AppState>)` — keep `AppState` fields public
- Extract payment info from request extensions: `Extension<Option<PaymentInfo>>`
- Middleware is layered bottom-up in `build_router` — innermost layer runs last
- Integration tests use `tower::ServiceExt::oneshot` — no live server needed
- Query params: `Query(params): Query<MyQueryStruct>` with `#[serde(default)]` fields

### Database

- PostgreSQL via `sqlx` — use `sqlx::query()`/`sqlx::query_as()` (runtime-checked)
- All DB writes are fire-and-forget: `tokio::spawn(async move { ... })` — never `.await` on the hot path
- UUID primary keys via `uuid::Uuid::new_v4()`
- Migration SQL lives in `migrations/` — always use `CREATE TABLE IF NOT EXISTS`

### Logging

- Use `tracing` (not `log`) — `tracing::info!`, `tracing::warn!`, `tracing::error!`
- Always use structured fields: `tracing::info!(wallet = %addr, model = %model, "processing request")`
- Set log level via `RUST_LOG` env var: `RUST_LOG=gateway=info,tower_http=info`

### Security

- Custom `Debug` impls redact all secrets (API keys, fee payer keys) — see `config.rs`
- Secrets come from env vars only, **never** config files

## Skills — Invoke Before Making Changes

These skills contain patterns, checklists, and constraints specific to this project's domains. Invoke them via the `Skill` tool BEFORE writing or modifying code in the matching areas.

| Skill | Invoke when touching | Key files |
|---|---|---|
| `solana-dev` | Solana, Anchor, SPL token, x402 protocol, escrow, fee payer, nonce, PDA, on-chain verification | `crates/x402/`, `programs/escrow/`, `solana.rs`, `fee_payer.rs`, `nonce_pool.rs`, `facilitator.rs` |
| `security-review` | Payment verification, crypto, API key handling, CORS, rate limiting, header decoding, secret redaction | `middleware/x402.rs`, `middleware/rate_limit.rs`, `config.rs` (redaction), `solana.rs` |
| `domain-fintech` | USDC calculations, atomic amounts, cost breakdowns, pricing, 5% fee logic, budget checks | `routes/chat/cost.rs`, `models.rs` (pricing), `usage.rs` (budgets) |
| `database-migrations` | Schema changes, new columns, indexes, PostgreSQL queries | `migrations/`, `usage.rs`, `wallet_budgets` |
| `postgres-patterns` | Query optimization, sqlx usage, connection pooling | `usage.rs`, `main.rs` (pool setup) |
| `domain-web` | Axum routes, middleware, Tower layers, SSE streaming, CORS, request/response handling | `routes/`, `middleware/`, `providers/`, `lib.rs` (`build_router`) |
| `m07-concurrency` | Async patterns, tokio::spawn, fire-and-forget, background tasks, Arc sharing | `usage.rs`, `cache.rs`, `balance_monitor.rs`, `escrow/claimer.rs`, `main.rs` |
| `api-design` | Adding/changing HTTP endpoints, 402 response shape, OpenAI compatibility, query params | `routes/chat.rs`, `routes/services.rs`, `routes/models.rs`, x402 types |
| `tdd-workflow` | Any new feature or bugfix — write tests first | All crates |
| `docker-patterns` | Container config, compose services, multi-stage builds | `Dockerfile`, `docker-compose.yml` |
| `deployment-patterns` | Deploy config, CI/CD, infrastructure | `Dockerfile`, `fly.toml` |

## Deployment

- Dockerfile: 3-stage build with cargo-chef for dependency caching
- Fly.io config in `fly.toml` (app: `solvela-gateway`, port 8402, region ord)
- Docker Compose for local dev: PostgreSQL 16 + Redis 7
- Dashboard: Next.js on Vercel (`solvela.vercel.app`)
- Migrations in `migrations/` run automatically on startup (idempotent `CREATE IF NOT EXISTS`)
