<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# elizaos

## Purpose
ElizaOS plugin that lets ElizaOS agents pay for LLM calls via Solvela. Exposes a `chat` action and a `gateway` provider that ElizaOS can invoke.

## Key Files
| File | Description |
|------|-------------|
| `package.json` | NPM manifest — ElizaOS plugin metadata |
| `tsconfig.json` | TypeScript compiler config |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Plugin source (see `src/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Conform to the ElizaOS plugin contract (`actions`, `providers`) — follow whatever version of `@elizaos/core` is pinned in `package.json`.
- Never move the wallet key into the plugin state — ElizaOS hosts the wallet; the plugin only asks it to sign.
- Keep the plugin zero-state — all payment context comes from the request.

### Testing Requirements
```bash
cd integrations/elizaos && npm install && npm test
```
(Use whatever script `package.json` actually defines; add one if missing.)

### Common Patterns
- Thin action wrapper around a Solvela HTTP call — handle 402 by asking ElizaOS to sign, then retry with `PAYMENT-SIGNATURE` header.

## Dependencies

### Internal
- Solvela gateway `/v1/chat/completions` + x402 flow.

### External
- `@elizaos/core` (or equivalent).

<!-- MANUAL: -->
