<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# crates

## Purpose
The Rust workspace. Contains all library and binary crates that make up the Solvela gateway and its supporting protocol + routing libraries. The workspace root is `../Cargo.toml`.

## Key Files
| File | Description |
|------|-------------|
| _(no loose files — see subdirectories)_ | Each crate lives in its own subdirectory |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `gateway/` | Axum HTTP server — the only binary (`solvela-gateway`) (see `gateway/AGENTS.md`) |
| `x402/` | Pure x402 payment protocol library — no Axum dep (see `x402/AGENTS.md`) |
| `router/` | Smart request router: rule-based scorer + model registry (see `router/AGENTS.md`) |
| `protocol/` | Shared wire-format types (`solvela-protocol`), zero deps on workspace (see `protocol/AGENTS.md`) |
| `cli/` | `solvela` CLI binary (wallet/chat/health/doctor) (see `cli/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Crate layering is load-bearing: `protocol` → `x402` / `router` → `gateway`. Never add reverse deps.
- `gateway` is the only binary. All other crates are libraries (`lib.rs` only).
- `x402` must remain free of Axum / web framework coupling — it's consumed by both the gateway and CLI.
- Edition is Rust 2021 (workspace `resolver = "2"`); a 2024 migration is planned but not done.

### Testing Requirements
```bash
cargo test                        # all workspace tests
cargo test -p gateway             # single crate
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

### Common Patterns
- Thiserror for library error enums; anyhow only in `main.rs` and tests.
- Never `unwrap()`/`expect()` in library code — propagate with `?`.
- Tokio runtime, Axum 0.8, sqlx (runtime-checked queries), tracing for logging.

## Dependencies

### Internal
- Root `Cargo.toml` defines `[workspace.dependencies]` — every crate references it via `= { workspace = true }`.

### External
- `axum`, `tokio`, `tower`, `sqlx`, `reqwest`, `tracing`, `serde`, `thiserror`, `ed25519-dalek`, `bs58`, `base64`.

<!-- MANUAL: -->
