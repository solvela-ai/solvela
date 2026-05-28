<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# actions

## Purpose
ElizaOS actions — units of work the agent can invoke. Each action is a standalone function plus metadata (name, similes, examples) that ElizaOS uses to decide when to call it.

## Key Files
| File | Description |
|------|-------------|
| `chat.ts` | The `CHAT_VIA_SOLVELA` action — sends the agent's current context to Solvela's `/v1/chat/completions`. On 402 it returns a human-readable payment-required message (cost summary). Wallet signing and sign-and-retry are not yet implemented — see the package README's Limitations section. |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Every action follows the ElizaOS action schema — name, similes, validate, handler, examples.
- Do not hold private keys; when wallet signing lands, use whatever signing hook ElizaOS exposes.
- Handle both 200 and 402 responses. Today the 402 path returns a cost summary; once wallet signing lands the action should sign and retry once, then propagate any remaining error to the agent runtime.

### Testing Requirements
```bash
npm --prefix integrations/elizaos test
```

### Common Patterns
- Return the final text via the `callback({ text })` shape ElizaOS expects. Streaming responses through ElizaOS is not yet wired up.

## Dependencies

### Internal
- The plugin's gateway URL setting (`SOLVELA_GATEWAY_URL`), read via `runtime.getSetting()`. The `../providers/gateway.ts` provider exposes liveness context to the runtime but is not a shared HTTP client.

### External
- `@elizaos/core`.

<!-- MANUAL: -->
