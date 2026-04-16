<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# protocol

## Purpose
Shared wire-format types for the Solvela ecosystem. The lowest crate in the workspace — zero dependencies on other workspace crates — so `x402`, `router`, `gateway`, and `cli` can all consume it. Contains: OpenAI-compatible chat types, x402 payment types, cost breakdowns, model info, streaming chunk envelopes, tool-use types, vision message parts, and shared constants (USDC mint, platform fee, protocol version). Crate name: `solvela-protocol`.

## Key Files
| File | Description |
|------|-------------|
| `Cargo.toml` | Minimal manifest — only `serde`, `serde_json`, `thiserror` |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Type definitions + flat re-exports (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- This crate MUST remain leaf-level — no dependency on any other workspace crate, ever.
- Types here are part of the public wire contract. Breaking changes require a deprecation cycle; prefer adding new fields with `#[serde(default)]` over renaming.
- Errors via `thiserror`. No anyhow. No logging.
- Ordering of derives: `Debug, Clone, Serialize, Deserialize` — Serde always last.

### Testing Requirements
```bash
cargo test -p solvela-protocol
```
Test payloads for wire compatibility: round-trip serde of every public struct.

### Common Patterns
- Flat re-exports via `pub use module::*` so consumers can `use solvela_protocol::{ChatRequest, PaymentRequired}`.
- Use `#[serde(rename_all = "snake_case")]` only when the wire format requires it — otherwise default.

## Dependencies

### Internal
_(none — leaf crate)_

### External
- `serde`, `serde_json`, `thiserror`.

<!-- MANUAL: -->
