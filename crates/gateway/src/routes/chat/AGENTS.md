<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# chat

## Purpose
The `POST /v1/chat/completions` pipeline, split by concern. `mod.rs` is the HTTP handler; the other files isolate cost calculation, payment enforcement, provider selection/proxying, and response shaping.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Route handler — orchestrates the full chat flow: resolve model → prompt-guard → cost calc → payment check (402 if missing) → provider proxy → cache → usage log |
| `cost.rs` | Computes provider cost + 5% platform fee, assembles `CostBreakdown`, converts to atomic USDC |
| `payment.rs` | 402 response assembly (list accepted schemes, include facilitator URL), replay-protection via Redis, payment validation entry point |
| `provider.rs` | Chooses the provider adapter + model for the resolved request (applies routing profile + fallback chain) |
| `response.rs` | Normalizes upstream responses back to OpenAI-compat shape; handles SSE vs. JSON |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- The 5% platform fee always applies — include the breakdown in every 402 response.
- Replay protection keys on `(wallet, tx_signature)` in Redis with a short TTL — set it before verification.
- If no provider API key is configured, the route falls back to a stub — keep that path intact for local dev.
- Streaming writes SSE chunks through `axum::response::sse::Sse` + `futures::Stream`.
- Usage logging is fire-and-forget via `tokio::spawn`; never await on a DB write while holding the request future.

### Testing Requirements
```bash
cargo test -p gateway chat
cargo test -p gateway --test integration
```

### Common Patterns
- Model resolution accepts aliases ("sonnet"), profiles ("auto"), or direct IDs.
- `CostBreakdown { provider, fee, total }` is the canonical cost shape.

## Dependencies

### Internal
- `crate::providers`, `crate::middleware::x402`, `crate::cache`, `crate::usage`, `crate::error`.
- `x402` for verification; `solvela-router` for scoring + model lookup; `solvela-protocol` for wire types.

### External
- `axum`, `serde`, `serde_json`, `tokio`, `futures`, `tracing`, `redis`.

<!-- MANUAL: -->
