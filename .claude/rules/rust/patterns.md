---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
---
# Rust Patterns

> This file extends [common/patterns.md](../common/patterns.md) with Rust specific content.

## Async Runtime

- Use **tokio** as the async runtime (`#[tokio::main]`, `#[tokio::test]`)
- Prefer `tokio::spawn` for background tasks
- Use `Arc<T>` for shared state, `tokio::sync::{Mutex, RwLock}` for mutation

## Observability

- Use `tracing` (not `log`) for structured logging
- Always use structured fields: `tracing::info!(wallet = %addr, "processing")`
- JSON output in production via `tracing-subscriber`

## Configuration

- Use `config` crate with typed `Config` struct
- Environment-specific overrides via `config/default.toml` + env vars

## Web Framework

- **Axum** for HTTP services with Tower middleware layers
- Route handlers return `Result<impl IntoResponse, AppError>`
- Share state with `State(Arc<AppState>)`

## Reference

See skills: `rust-router`, `rust-pro`, `rust-async-patterns` for comprehensive patterns.
