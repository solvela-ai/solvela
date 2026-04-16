<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# providers

## Purpose
ElizaOS providers — injected context / clients that actions can use. Hosts the pre-configured Solvela gateway HTTP client here so actions stay thin.

## Key Files
| File | Description |
|------|-------------|
| `gateway.ts` | Gateway provider — returns an HTTP client configured against the Solvela base URL, including x402 request/response handling |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Base URL is configurable via an env var (e.g., `SOLVELA_GATEWAY_URL`) with a sensible default.
- Provider must be stateless across invocations; no in-memory caching of signed payments.
- Expose a typed client (not a raw `fetch`) so actions get compile-time guarantees on request shape.

### Testing Requirements
```bash
npm --prefix integrations/elizaos test
```

### Common Patterns
- Use `fetch` (Node 20+) or an injected `undici`-style client; avoid heavy axios/superagent.

## Dependencies

### Internal
_(none — provider is a leaf module)_

### External
- `@elizaos/core`.

<!-- MANUAL: -->
