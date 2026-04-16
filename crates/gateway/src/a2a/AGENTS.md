<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# a2a

## Purpose
A2A (Agent-to-Agent) protocol adapter. Translates A2A JSON-RPC `message/send` calls into the existing x402 + chat pipeline. This is a **protocol adapter, not new payment logic** — no fiat, no AP2 mandates, no card processing.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; re-exports public types and the top-level handler |
| `types.rs` | A2A wire types: `Task`, `Message`, `Artifact`, `TaskState`, x402/AP2 metadata |
| `agent_card.rs` | Builds the `AgentCard` returned at `GET /.well-known/agent.json` (includes AP2 + x402 extensions) |
| `jsonrpc.rs` | JSON-RPC 2.0 request/response envelope + error mapping |
| `handler.rs` | Main `message/send` handler — cost calc → 402 / payment verify → LLM proxy → Task response |
| `task_store.rs` | In-memory task store keyed by task id — tracks state transitions across request chains |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- A2A flow:
  1. Agent calls `GET /.well-known/agent.json` → gets `AgentCard`.
  2. Agent sends `message/send` (no payment) → gateway returns a Task in `input-required` state with `x402.payment.required` metadata.
  3. Agent signs SPL USDC transfer → sends `message/send` with `taskId` + `x402.payment.payload` metadata.
  4. Gateway verifies payment (reuses the facilitator), proxies to LLM, returns Task in `completed` state with artifacts + receipt.
- Do **not** add new payment schemes here — reuse the gateway's x402 middleware and facilitator.
- Do **not** persist tasks in PostgreSQL unless the gateway lifecycle already does so — today the store is in-memory.

### Testing Requirements
```bash
cargo test -p gateway a2a
```

### Common Patterns
- JSON-RPC errors use the codes defined in `jsonrpc.rs`.
- Task state machine: `submitted → input-required → working → completed` (with `failed` / `canceled` as terminal).

## Dependencies

### Internal
- `crate::routes::chat` for cost calculation and provider proxy.
- `crate::middleware::x402` for payment decoding / verification.

### External
- `axum`, `serde`, `serde_json`, `tracing`, `uuid`.

<!-- MANUAL: -->
