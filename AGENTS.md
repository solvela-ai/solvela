# AGENTS.md — RustyClawRouter

Guidelines for AI coding agents operating in this repository.

## Project Overview

Solana-native AI agent payment infrastructure (Rust/Axum). AI agents pay for LLM
API calls with USDC-SPL on Solana via the x402 protocol. No API keys, just wallets.

Architecture: Cargo workspace with crates under `crates/` (gateway, x402, router, common, cli).
Read `.claude/plan/rustyclawrouter.md` for the full implementation plan before making
architectural decisions.

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

# Check without building (faster — prefer this for iteration)
cargo check
cargo check -p gateway
```

## Test Commands

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p gateway
cargo test -p x402
cargo test -p router
cargo test -p rcr-common

# Run a single test by exact name
cargo test -p gateway test_health_endpoint -- --exact
cargo test -p router test_weights_sum_to_one -- --exact

# Run tests matching a pattern (substring match across all crates)
cargo test scorer
cargo test payment

# Run integration tests only (crates/gateway/tests/)
cargo test -p gateway --test integration

# Run unit tests only (inline #[cfg(test)] modules)
cargo test -p gateway --lib

# Show stdout from tests (useful for tracing output)
cargo test -p x402 -- --nocapture

# Run tests with a specific thread count
cargo test -- --test-threads=1
```

## Lint & Format

```bash
# Format all code — must pass CI
cargo fmt --all

# Check formatting without writing (CI check mode)
cargo fmt --all -- --check

# Clippy lints — must pass CI (warnings are errors)
cargo clippy --all-targets --all-features -- -D warnings

# Run both together before committing
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

## Code Style

### Rust Conventions

- **Edition**: Rust 2021 (`resolver = "2"` workspace)
- **Async runtime**: Tokio — use `#[tokio::main]` and `#[tokio::test]`
- **Web framework**: Axum 0.8 with Tower middleware layers
- **Error handling**:
  - `thiserror` for all library/crate-level error enums
  - `anyhow` only in binary entry points (`main.rs`) and tests
  - Never use `unwrap()` or `expect()` in library code — always propagate with `?`
  - Match on `Result`/`Option` only when branching is needed; use `?` for propagation
- **Naming**:
  - Types/traits: `PascalCase` — `PaymentRequired`, `LLMProvider`, `ChatResponse`
  - Functions/methods: `snake_case` — `verify_payment`, `chat_completion`
  - Constants: `SCREAMING_SNAKE_CASE` — `USDC_MINT`, `MAX_TIMEOUT_SECONDS`
  - Modules/files: `snake_case` — `rate_limit.rs`, `smart_router.rs`
  - Crate names: `kebab-case` in Cargo.toml, `snake_case` in `use` statements

### Import Ordering

Separate groups with a blank line; `rustfmt` enforces this:

```rust
use std::sync::Arc;                        // 1. Standard library

use axum::{Router, routing::post};         // 2. External crates (alphabetical)
use serde::{Deserialize, Serialize};
use tracing::info;

use rcr_common::types::ChatRequest;        // 3. Workspace crates

use crate::config::AppConfig;             // 4. Crate-internal modules
```

### Struct & Enum Patterns

- Derive order: `Debug, Clone, Serialize, Deserialize` — Serde always last
- Error enums: `#[derive(Debug, thiserror::Error)]` with `#[error("...")]` on every variant
- Async trait methods: always annotate with `#[async_trait::async_trait]`
- Thread-safe traits: bound with `Send + Sync` (required for Axum `State`)
- Config structs: `#[derive(Debug, Clone, Deserialize)]` — no `Serialize` unless needed
- Use `#[serde(default)]` on optional config fields with `Option<T>`

### Axum Patterns

- Route handlers return `Result<impl IntoResponse, GatewayError>`
- Share state with `State(Arc<AppState>)` — keep `AppState` fields public
- Extract payment info from request extensions: `Extension<Option<PaymentInfo>>`
- Middleware is layered bottom-up in `build_router` — innermost layer runs last
- Integration tests use `tower::ServiceExt::oneshot` — no live server needed

### Configuration

- Config files are TOML: `config/models.toml`, `config/default.toml`, `config/services.toml`
- Use the `config` crate for layered config (file + env var overrides)
- Env var prefix: `RCR_` for gateway config (e.g., `RCR_SERVER_PORT`)
- Secrets (wallet keys, provider API keys) come from env vars, **never** config files
- See `.env.example` for all required environment variables

### Database

- PostgreSQL via `sqlx` with compile-time checked queries (`query!` macro)
- All DB writes are async in `tokio::spawn` — never block the request path
- Use raw SQL, no ORM
- UUID primary keys via `uuid::Uuid::new_v4()`

### Logging / Tracing

- Use `tracing` (not `log`) — `tracing::info!`, `tracing::warn!`, `tracing::error!`
- Always use structured fields: `tracing::info!(wallet = %addr, model = %model, "processing request")`
- Set log level via `RUST_LOG` env var: `RUST_LOG=gateway=info,tower_http=info`
- Debug-level logs are allowed inside hot paths (scorer, middleware) — keep them cheap

## Project Structure

```
crates/
  gateway/src/          Axum HTTP server — the only binary in the workspace
    main.rs             Binary entry point
    lib.rs              Exposes internals for integration tests
    config.rs           AppConfig, SolanaConfig, ServerConfig
    error.rs            GatewayError (thiserror) → IntoResponse
    cache.rs            Redis response cache (optional)
    usage.rs            Usage tracking (async fire-and-forget to Postgres)
    middleware/
      x402.rs           Payment header extraction (does NOT enforce payment)
      rate_limit.rs     Token-bucket rate limiter (in-memory)
    providers/
      mod.rs            LLMProvider trait + ProviderRegistry + SSE parser
      health.rs         Circuit breaker + ProviderHealthTracker
      openai.rs         OpenAI adapter
      anthropic.rs      Anthropic adapter
      google.rs         Google Gemini adapter
      xai.rs            xAI Grok adapter
      deepseek.rs       DeepSeek adapter
      fallback.rs       Stub provider (no API key configured)
    routes/
      chat.rs           POST /v1/chat/completions — 402 + routing + proxy
      models.rs         GET /v1/models — lists models with USDC pricing
      health.rs         GET /health
  gateway/tests/
    integration.rs      In-process HTTP tests via tower::ServiceExt::oneshot
  x402/src/             x402 protocol library — NO Axum dependency
    types.rs            PaymentPayload, PaymentAccept, SolanaPayload, etc.
    traits.rs           PaymentVerifier trait (chain-agnostic)
    solana.rs           SolanaVerifier implementation
    solana_types.rs     Solana transaction deserialization types
    facilitator.rs      Facilitator — routes verification to correct verifier
  router/src/           Smart routing engine
    scorer.rs           15-dimension rule-based request complexity scorer
    profiles.rs         Routing profiles (eco, balanced, performance, reasoning)
    models.rs           ModelRegistry — loads models.toml
  common/src/           Shared types across all crates
    types.rs            ChatRequest, ChatResponse, ChatMessage, ModelInfo, etc.
    error.rs            Shared error utilities
  cli/src/              rcr CLI binary
    main.rs             CLI entry point (clap derive)
    commands/           Subcommand implementations
config/
  models.toml           Model registry + per-token pricing
  default.toml          Gateway defaults (host, port, Solana RPC)
  services.toml         x402 service marketplace registry
sdks/
  python/               pip install rustyclawrouter
  typescript/           npm install @rustyclawrouter/sdk
  go/                   go get github.com/rustyclawrouter/sdk-go
```

## Key Architectural Rules

1. **`gateway` is the only binary** — all other crates are libraries (`lib.rs` only)
2. **`x402` has no Axum dependency** — pure protocol library, no HTTP framework coupling
3. **`PaymentVerifier` trait is chain-agnostic** — designed for future EVM/Base support
4. **Provider adapters translate OpenAI ↔ native format** — gateway always speaks OpenAI format
5. **5% platform fee on all requests** — always include a `cost_breakdown` in payment info
6. **Solana-first** — Base/EVM is a future feature; do not implement EVM paths now
7. **Never store private keys** — wallet keys stay client-side; only signed txs reach the gateway
8. **Payment middleware extracts, routes enforce** — `x402.rs` middleware never returns 402; that is the route handler's responsibility
9. **All DB writes are fire-and-forget** — wrap in `tokio::spawn`; never `.await` on the hot path
10. **Integration tests need no live server** — use `tower::ServiceExt::oneshot` with `test_app()`
