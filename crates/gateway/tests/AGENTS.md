<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
Gateway integration tests. Drive the full router (middleware + routes) in-process via `tower::ServiceExt::oneshot` — no live HTTP server, no network, no docker compose required. Optional: a `DATABASE_URL` / `REDIS_URL` may enable DB- and cache-backed paths.

## Key Files
| File | Description |
|------|-------------|
| `integration.rs` | Main integration suite — boots `test_app()`, exercises routes end-to-end |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Use `test_app()` / similar fixture from `gateway::lib` to get a pre-wired `axum::Router`.
- Drive requests with `router.oneshot(Request::builder()…)` from `tower`.
- Assert both status code AND response body shape — OpenAI-compatibility is load-bearing.
- PostgreSQL + Redis are optional; gate tests that need them with `#[cfg(feature = "…")]` or runtime skip if the env var is absent.

### Testing Requirements
```bash
cargo test -p gateway --test integration
cargo test -p gateway --test integration -- --nocapture
```

### Common Patterns
- Build request bodies with `serde_json::json!({…})` and `Body::from(body.to_string())`.
- Decode responses with `hyper::body::to_bytes` + `serde_json::from_slice`.
- New route? Add at least: success case, 402 when unpaid, 4xx on bad input.

## Dependencies

### Internal
- `gateway` (lib target) — import `gateway::build_router`, `gateway::AppState`, etc.

### External
- `tower`, `axum`, `hyper`, `serde_json`, `tokio` (via `#[tokio::test]`).

<!-- MANUAL: -->
