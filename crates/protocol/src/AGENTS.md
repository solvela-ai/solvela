<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
Type definitions for the Solvela public wire format. Every module is flat-re-exported from `lib.rs` so consumers write `use solvela_protocol::{ChatRequest, PaymentRequired, CostBreakdown}`.

## Key Files
| File | Description |
|------|-------------|
| `lib.rs` | Module declarations + flat `pub use` re-exports |
| `chat.rs` | OpenAI-compatible chat request / response / message types |
| `constants.rs` | USDC mint, USDC decimals, platform-fee percent, x402 version, Solana network IDs |
| `cost.rs` | `CostBreakdown` (provider cost + platform fee + total), pricing helpers |
| `model.rs` | `ModelInfo` — id, display name, context window, pricing, capability flags |
| `payment.rs` | `PaymentRequired` (402 body), `PaymentPayload` (header body), `PaymentScheme`, signatures |
| `settlement.rs` | Settlement result (success, tx signature, refund info) |
| `streaming.rs` | SSE / chunked streaming envelope for chat completions |
| `tools.rs` | Tool-use types (function definitions, tool calls, tool results) |
| `vision.rs` | Multi-modal message parts (image URLs, inline data) |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- These are public wire types. Adding a field: use `#[serde(default)]` or `Option<T>` so existing clients keep working. Renaming: bump a version, do not silently break.
- Derive order: `Debug, Clone, Serialize, Deserialize` (Serde last).
- Use `#[serde(rename_all = "snake_case")]` only when the spec requires it — otherwise rely on default camelCase-via-Serde.
- Keep modules small — one logical concept per file.

### Testing Requirements
```bash
cargo test -p solvela-protocol
```
Every struct should have a round-trip JSON serde test.

### Common Patterns
- Atomic USDC amounts (u64, 6 decimals) — never f64. Helper types wrap raw u64 when clarity matters.
- Error enums via `thiserror` — no `anyhow` in this crate.
- No async, no tokio, no logging — pure types + serde.

## Dependencies

### Internal
_(none — leaf crate)_

### External
- `serde`, `serde_json`, `thiserror`.

<!-- MANUAL: -->
