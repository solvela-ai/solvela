<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# actions

## Purpose
ElizaOS actions — units of work the agent can invoke. Each action is a standalone function plus metadata (name, similes, examples) that ElizaOS uses to decide when to call it.

## Key Files
| File | Description |
|------|-------------|
| `chat.ts` | The `chat` action — sends the agent's current context to Solvela's `/v1/chat/completions`, handles the 402 by asking ElizaOS to sign, then retries with the `PAYMENT-SIGNATURE` header |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Every action follows the ElizaOS action schema — name, similes, validate, handler, examples.
- Do not hold private keys; use whatever signing hook ElizaOS exposes.
- Handle both 200 and 402 responses. On 402, sign and retry once; propagate any remaining error to the agent runtime.

### Testing Requirements
```bash
npm --prefix integrations/elizaos test
```

### Common Patterns
- Stream responses back to ElizaOS when the user requested streaming; otherwise return the final text.

## Dependencies

### Internal
- Plugin providers (`../providers`) for the gateway HTTP client.

### External
- `@elizaos/core`.

<!-- MANUAL: -->
