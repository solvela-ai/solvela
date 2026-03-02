# AGENTS.md — RustyClawRouter

Guidelines for AI coding agents operating in this repository.

## Project Overview

Solana-native AI agent payment infrastructure (Rust/Axum). AI agents pay for LLM
API calls with USDC-SPL on Solana via the x402 protocol. No API keys, just wallets.

Architecture: Cargo workspace with crates under `crates/` (gateway, x402, router, common, cli).
Read `.claude/plan/rustyclawrouter.md` for the full implementation plan before making
architectural decisions. Phases 1–6 are complete; see "What's Done" below.

**Installed skills** — load these when working in their domain:
- Solana/Anchor/x402 work → `/skillsinit solana-dev`
- Go SDK work → load `golang-patterns` skill

---

## Build Commands

```bash
# Build entire workspace
cargo build

# Build specific crate
cargo build -p gateway
cargo build -p x402
cargo build -p router
cargo build -p rcr-common
cargo build -p rcr-cli

# Release build
cargo build --release

# Check without building (faster — prefer for iteration)
cargo check
cargo check -p gateway
```

## Test Commands

```bash
# Run all tests (139 tests across all crates)
cargo test

# Run tests for a specific crate
cargo test -p gateway       # 77 tests (56 unit + 21 integration)
cargo test -p x402          # 39 tests
cargo test -p router        # 13 tests
cargo test -p rcr-common    # 10 tests

# Run a single test by EXACT name
cargo test -p gateway test_health_endpoint -- --exact
cargo test -p gateway test_services_filter_by_category -- --exact
cargo test -p router test_weights_sum_to_one -- --exact

# Run tests matching a pattern (substring match across all crates)
cargo test scorer
cargo test payment
cargo test services

# Run integration tests only (crates/gateway/tests/)
cargo test -p gateway --test integration

# Run unit tests only (inline #[cfg(test)] modules)
cargo test -p gateway --lib

# Show stdout (useful for tracing output)
cargo test -p x402 -- --nocapture

# Anchor escrow program (standalone — NOT in workspace)
cargo test --manifest-path programs/escrow/Cargo.toml

# Dashboard Vitest tests
npm --prefix dashboard test

# Go SDK tests
go test ./... -v
```

## Lint & Format

```bash
# Format + lint — MUST pass before committing
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings

# Check-only (CI mode)
cargo fmt --all -- --check

# Dashboard lint
npm --prefix dashboard run lint
```

## Local Dev Stack

```bash
# Start PostgreSQL + Redis (required for spend logging and response cache)
docker compose up -d

# Stop
docker compose down

# Full reset (deletes volumes)
docker compose down -v

# Copy env template and fill in at least one provider API key
cp .env.example .env

# Run gateway
RUST_LOG=info cargo run -p gateway
# Gateway listens on http://localhost:8402
```

Migrations in `migrations/` are applied automatically by `docker compose up` and also
run on gateway startup via `run_migrations()` in `main.rs` (idempotent).

---

## Code Style

### Rust Conventions

- **Edition**: Rust 2021 (`resolver = "2"` workspace)
- **Async runtime**: Tokio — use `#[tokio::main]` and `#[tokio::test]`
- **Web framework**: Axum 0.8 with Tower middleware layers
- **Error handling**:
  - `thiserror` for all library/crate-level error enums
  - `anyhow` only in binary entry points (`main.rs`) and tests
  - Never use `unwrap()` or `expect()` in library code — propagate with `?`
  - Match on `Result`/`Option` only when branching is needed
- **Naming**:
  - Types/traits: `PascalCase` — `PaymentRequired`, `ServiceRegistry`
  - Functions/methods: `snake_case` — `verify_payment`, `list_services`
  - Constants: `SCREAMING_SNAKE_CASE` — `USDC_MINT`, `PLATFORM_FEE_PERCENT`
  - Modules/files: `snake_case` — `rate_limit.rs`, `services.rs`
  - Crate names: `kebab-case` in Cargo.toml, `snake_case` in `use` statements

### Import Ordering

Separate groups with a blank line; `rustfmt` enforces this:

```rust
use std::sync::Arc;                          // 1. Standard library

use axum::{Router, routing::{get, post}};   // 2. External crates (alphabetical)
use serde::{Deserialize, Serialize};
use tracing::info;

use rcr_common::services::ServiceRegistry;  // 3. Workspace crates

use crate::config::AppConfig;               // 4. Crate-internal modules
```

### Struct & Enum Patterns

- Derive order: `Debug, Clone, Serialize, Deserialize` — Serde always last
- Error enums: `#[derive(Debug, thiserror::Error)]` with `#[error("...")]` on every variant
- Async trait methods: annotate with `#[async_trait::async_trait]`
- Thread-safe traits: bound with `Send + Sync` (required for Axum `State`)
- Config structs: `#[derive(Debug, Clone, Deserialize)]` — no `Serialize` unless needed
- Use `#[serde(default)]` on optional config fields with `Option<T>`

### Axum Patterns

- Route handlers return `Result<impl IntoResponse, GatewayError>`
- Share state with `State(Arc<AppState>)` — keep `AppState` fields public
- Extract payment info from request extensions: `Extension<Option<PaymentInfo>>`
- Middleware is layered bottom-up in `build_router` — innermost layer runs last
- Integration tests use `tower::ServiceExt::oneshot` — no live server needed
- Query params: `Query(params): Query<MyQueryStruct>` with `#[serde(default)]` fields

### Configuration

- Config files are TOML: `config/models.toml`, `config/default.toml`, `config/services.toml`
- Env var prefix: `RCR_` for gateway config (e.g., `RCR_SERVER_PORT`)
- Secrets (wallet keys, provider API keys) come from env vars, **never** config files
- See `.env.example` for all required environment variables

### Database

- PostgreSQL via `sqlx` — use `sqlx::query()`/`sqlx::query_as()` (runtime-checked)
- All DB writes are fire-and-forget: `tokio::spawn(async move { ... })` — never `.await` on the hot path
- UUID primary keys via `uuid::Uuid::new_v4()`
- Migration SQL lives in `migrations/` — always use `CREATE TABLE IF NOT EXISTS`

### Logging / Tracing

- Use `tracing` (not `log`) — `tracing::info!`, `tracing::warn!`, `tracing::error!`
- Always use structured fields: `tracing::info!(wallet = %addr, model = %model, "processing request")`
- Set log level via `RUST_LOG` env var: `RUST_LOG=gateway=info,tower_http=info`

### Anchor / Solana (programs/escrow)

- The escrow program is a **standalone Cargo project** — it is NOT a workspace member.
  This avoids `thiserror` v1/v2 and `base64` version conflicts with the gateway workspace.
- Use `#[derive(InitSpace)]` on account structs; space = `8 + MyAccount::INIT_SPACE`
- PDA seeds follow `[b"escrow", agent_pubkey.as_ref(), &service_id]`
- Test with LiteSVM (fast, in-process) — see the `solana-dev` skill for full testing patterns
- USDC mint: `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` (mainnet), devnet has own mint
- To build escrow tests: `OPENSSL_NO_PKG_CONFIG=1 OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test --manifest-path programs/escrow/Cargo.toml`

---

## Project Structure

```
crates/
  gateway/src/          Axum HTTP server — the only binary in the workspace
    main.rs             Entry point; connects PostgreSQL + Redis on startup
    lib.rs              Exposes AppState + build_router for integration tests
    config.rs           AppConfig, SolanaConfig, ServerConfig
    error.rs            GatewayError (thiserror) → IntoResponse
    cache.rs            Redis response cache + replay-protection (ResponseCache)
    usage.rs            Usage tracking → PostgreSQL (fire-and-forget writes)
    middleware/
      x402.rs           Extracts PAYMENT-SIGNATURE header (does NOT enforce)
      rate_limit.rs     Token-bucket rate limiter (in-memory)
      prompt_guard.rs   Injection/jailbreak/PII detection middleware
    providers/
      mod.rs            LLMProvider trait + ProviderRegistry + SSE parser
      health.rs         Circuit breaker + ProviderHealthTracker
      openai.rs / anthropic.rs / google.rs / xai.rs / deepseek.rs / fallback.rs
    routes/
      chat.rs           POST /v1/chat/completions — 402 + routing + proxy
      images.rs         POST /v1/images/generations
      models.rs         GET  /v1/models — lists models with USDC pricing
      services.rs       GET  /v1/services — x402 service marketplace (Phase 6)
      pricing.rs        GET  /pricing — per-model pricing + fee breakdown
      supported.rs      GET  /v1/supported — supported models list
      health.rs         GET  /health
  gateway/tests/
    integration.rs      21 in-process HTTP tests via tower::ServiceExt::oneshot
  x402/src/             x402 protocol library — NO Axum dependency
    types.rs            PaymentPayload, PaymentAccept, SolanaPayload, etc.
    traits.rs           PaymentVerifier trait (chain-agnostic, future EVM)
    solana.rs           SolanaVerifier — tx decode + SPL transfer validation
    solana_types.rs     Solana transaction deserialization (no solana-sdk dep)
    facilitator.rs      Routes verification to correct verifier by network
  router/src/           Smart routing engine
    scorer.rs           15-dimension rule-based request complexity scorer
    profiles.rs         Routing profiles (eco, balanced, performance, reasoning)
    models.rs           ModelRegistry — loads models.toml
  common/src/           Shared types across all crates
    types.rs            ChatRequest, ChatResponse, ChatMessage, ModelInfo, etc.
    services.rs         ServiceRegistry + ServiceEntry — loads services.toml
    error.rs            Shared error utilities
  cli/src/              rcr CLI binary (clap derive)
    commands/           wallet, chat, models, health, stats, doctor
programs/
  escrow/               Anchor escrow program (standalone Cargo project)
    src/lib.rs          Program entry point + declare_id!
    src/state.rs        Escrow account struct (#[derive(InitSpace)])
    src/errors.rs       EscrowError enum (#[error_code])
    src/instructions/   deposit.rs / claim.rs / refund.rs
    tests/unit.rs       6 unit tests (INIT_SPACE, PDA derivation)
config/
  models.toml           Model registry + per-token pricing
  default.toml          Gateway defaults (host, port, Solana RPC)
  services.toml         x402 service marketplace registry (4 services)
migrations/
  001_initial_schema.sql  spend_logs + wallet_budgets tables (idempotent)
dashboard/              Next.js 16 + Tailwind CSS v4 + Recharts
  src/app/              overview, usage, models, wallet, settings pages
  src/lib/              api.ts (fetchHealth/fetchPricing/fetchModels), utils.ts
  src/types/            TypeScript type definitions
  src/__tests__/        61 Vitest tests (utils, mock-data, api)
sdks/
  python/               pip install rustyclawrouter (63 tests)
  typescript/           npm install @rustyclawrouter/sdk (19 tests)
  go/                   go get github.com/rustyclawrouter/sdk-go (18 tests)
  mcp/                  npx @rustyclawrouter/mcp (17 tests)
```

---

## Key Architectural Rules

1. **`gateway` is the only binary** — all other crates are libraries (`lib.rs` only)
2. **`x402` has no Axum dependency** — pure protocol library, no HTTP framework coupling
3. **`PaymentVerifier` trait is chain-agnostic** — designed for future EVM/Base support
4. **Provider adapters translate OpenAI ↔ native format** — gateway always speaks OpenAI format
5. **5% platform fee on all requests** — always include `cost_breakdown` in payment info
6. **Solana-first** — Base/EVM is a future feature; do not implement EVM paths now
7. **Never store private keys** — wallet keys stay client-side; only signed txs reach the gateway
8. **Payment middleware extracts, routes enforce** — `x402.rs` middleware never returns 402
9. **All DB writes are fire-and-forget** — `tokio::spawn`; never `.await` on the hot path
10. **Integration tests need no live server** — use `tower::ServiceExt::oneshot` with `test_app()`
11. **Escrow program is NOT a workspace member** — avoids dep version conflicts; build separately
12. **Both PostgreSQL and Redis are optional** — gateway degrades gracefully when either is absent
