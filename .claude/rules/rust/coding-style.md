---
paths:
  - "**/*.rs"
  - "**/Cargo.toml"
  - "**/Cargo.lock"
---
# Rust Coding Style

> This file extends [common/coding-style.md](../common/coding-style.md) with Rust specific content.

## Formatting

- **rustfmt** is mandatory — no style debates
- Run `cargo fmt --all` before committing

## Error Handling

- `thiserror` for library/crate-level error enums
- `anyhow::Result<T>` only in binary entry points and tests
- Never use `.unwrap()` or `.expect()` in library code — propagate with `?`

```rust
// WRONG
let val = map.get("key").unwrap();

// CORRECT
let val = map.get("key").ok_or_else(|| anyhow!("missing key"))?;
```

## Immutability

Rust enforces immutability by default. Respect it:
- Prefer `let` over `let mut` wherever possible
- Use `&T` references before reaching for `&mut T`
- Prefer returning new values over mutating in place

## Naming Conventions

- Types/traits: `PascalCase` — `PaymentRequired`, `ServiceRegistry`
- Functions/methods: `snake_case` — `verify_payment`, `list_services`
- Constants: `SCREAMING_SNAKE_CASE` — `MAX_RETRIES`, `DEFAULT_TIMEOUT`
- Modules/files: `snake_case` — `rate_limit.rs`, `services.rs`
- Crate names: `kebab-case` in Cargo.toml, `snake_case` in `use` statements

## Import Ordering

Separate groups with a blank line; `rustfmt` enforces this:

```rust
use std::sync::Arc;                          // 1. Standard library

use axum::{Router, routing::{get, post}};   // 2. External crates (alphabetical)
use serde::{Deserialize, Serialize};

use crate::config::AppConfig;               // 3. Crate-internal modules
```

## Derive Order

Standard order: `Debug, Clone, Serialize, Deserialize` — Serde always last.

## Reference

See skill: `rust-pro` for comprehensive Rust patterns and idioms.
