# AGENTS.md — RustyClawRouter

Guidelines for AI coding agents operating in this repository.

## Project Overview

Solana-native AI agent payment infrastructure (Rust/Axum). AI agents pay for LLM
API calls with USDC-SPL on Solana via the x402 protocol. No API keys, just wallets.

Architecture: Cargo workspace with crates under `crates/` (gateway, x402, router, common),
Anchor program under `programs/escrow/`, SDKs under `sdks/` (python, typescript, go).

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

# Release build
cargo build --release

# Check without building (faster)
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

# Run a single test by name
cargo test -p gateway test_chat_completions
cargo test -p x402 test_verify_solana_payment -- --exact

# Run tests matching a pattern
cargo test -p router scorer

# Show stdout from tests
cargo test -p x402 -- --nocapture

# Run only integration tests
cargo test --test integration

# Anchor program tests (from programs/escrow/)
cargo test-sbf          # SBF build + test
```

## Lint & Format

```bash
# Format all code (must pass CI)
cargo fmt --all

# Check formatting without writing
cargo fmt --all -- --check

# Clippy lints (must pass CI — treat warnings as errors)
cargo clippy --all-targets --all-features -- -D warnings

# Both together
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

## Code Style

### Rust Conventions

- **Edition**: Rust 2021
- **Async runtime**: Tokio (always `#[tokio::main]` / `#[tokio::test]`)
- **Framework**: Axum 0.8 with Tower middleware layers
- **Error handling**:
  - `thiserror` for library/crate-level error enums
  - `anyhow` only in binary entry points (`main.rs`) and tests
  - Always use `Result<T, Error>` — never `unwrap()` or `expect()` in library code
  - Use `?` operator for propagation, not manual `match` on `Result`
- **Naming**:
  - Types/traits: `PascalCase` — `PaymentRequired`, `LLMProvider`, `ChatResponse`
  - Functions/methods: `snake_case` — `verify_payment`, `chat_completion`
  - Constants: `SCREAMING_SNAKE_CASE` — `USDC_MINT`, `MAX_TIMEOUT_SECONDS`
  - Modules/files: `snake_case` — `rate_limit.rs`, `smart_router.rs`
  - Crate names: kebab-case in Cargo.toml, snake_case in `use` statements

### Import Ordering

Group imports in this order, separated by blank lines:

```rust
use std::sync::Arc;                    // 1. Standard library

use axum::{Router, routing::post};     // 2. External crates
use serde::{Deserialize, Serialize};

use rcr_common::types::ChatRequest;    // 3. Workspace crates

use crate::config::AppConfig;          // 4. Crate-internal modules
```

### Struct & Enum Patterns

- Derive order: `Debug, Clone, Serialize, Deserialize` (Serde last)
- Error enums use `#[derive(Debug, thiserror::Error)]` with `#[error("...")]` messages
- Always use `#[async_trait::async_trait]` for async trait methods
- Traits that cross threads: always bound with `Send + Sync`

### Configuration

- Config files are TOML: `config/models.toml`, `config/default.toml`, `config/services.toml`
- Use the `config` crate for layered config (file + env vars)
- Secrets (wallet keys, provider API keys) come from env vars, never config files

### Database

- PostgreSQL via `sqlx` with compile-time query checking
- All DB writes are async (`tokio::spawn`) — never on the request critical path
- Use raw SQL, not an ORM

### Logging

- Use `tracing` (not `log`) — `tracing::info!`, `tracing::error!`, etc.
- Structured fields: `tracing::info!(wallet = %addr, model = %model, "processing request")`

## Project Structure

```
crates/
  gateway/src/          Axum HTTP server (routes/, middleware/, providers/)
  x402/src/             x402 protocol (types, solana verification, facilitator)
  router/src/           Smart routing engine (scorer, profiles, models)
  common/src/           Shared types and utilities
programs/
  escrow/src/           Anchor escrow program (deposit/claim/refund)
sdks/
  python/               pip install rustyclawrouter
  typescript/           npm install @rustyclawrouter/sdk
  go/                   go get github.com/rustyclawrouter/sdk-go
config/
  models.toml           Model registry + pricing
  default.toml          Gateway configuration
  services.toml         x402 service marketplace registry
```

## Key Architectural Rules

1. **The gateway crate is the only binary** — all other crates are libraries
2. **x402 crate has no Axum dependency** — it's a pure protocol library
3. **PaymentVerifier trait in x402** is chain-agnostic (designed for future multi-chain)
4. **Provider adapters** translate OpenAI format to/from each provider's native format
5. **5% platform fee** on all proxied requests — always show cost breakdown transparently
6. **Solana-first** — Base/EVM is a future feature, not in the active implementation
7. **Never store private keys** — wallet keys stay client-side, only signed txs reach the gateway
