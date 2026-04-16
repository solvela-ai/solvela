<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# sdks

## Purpose
Client SDKs so agents in different languages/runtimes can talk to the Solvela gateway. Each SDK: loads a local wallet, signs SPL-USDC transactions client-side, constructs x402 headers, retries on 402, and surfaces OpenAI-compatible chat responses.

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `go/` | Go SDK — signing support in progress (see `go/AGENTS.md`) |
| `typescript/` | TypeScript/Node SDK (see `typescript/AGENTS.md`) |
| `python/` | Python SDK (see `python/AGENTS.md`) |
| `mcp/` | MCP server exposing Solvela as tools to MCP clients (see `mcp/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- **Private keys never leave the client.** SDKs sign locally; the gateway only sees signed transactions.
- Each SDK stays independent — no shared codegen today, but API surfaces are intentionally parallel so features port easily.
- x402 contract: send request → receive 402 with `payment_required` body → sign USDC-SPL transfer → retry with `PAYMENT-SIGNATURE: <base64-or-json>` header.
- 5% platform fee is already baked into the 402 response's `cost_breakdown` — SDKs surface it, don't recompute it.

### Testing Requirements
Each SDK has its own test runner — see its `AGENTS.md`.

### Common Patterns
- Minimal public surface: `Client`, `Wallet`, `chat()`.
- Errors typed per-language (Go `errors.Is`, Python custom exceptions, TS discriminated unions).

## Dependencies

### Internal
- Solvela gateway HTTP contract; shared wire types live in `../crates/protocol/` (SDKs reimplement these per-language).

### External
- Per-language HTTP and crypto libraries (see each SDK's AGENTS.md).

<!-- MANUAL: -->
