<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# providers

## Purpose
ElizaOS providers — context strings ElizaOS injects into prompts so the agent runtime knows what's available. The gateway provider tells the runtime whether the configured Solvela gateway is reachable.

## Key Files
| File | Description |
|------|-------------|
| `gateway.ts` | Gateway provider — `get()` resolves the `SOLVELA_GATEWAY_URL` setting (default `http://localhost:8402`), pings `/health`, and returns a short status string for the runtime (online / non-200 / unreachable). It is not an HTTP client for actions to call. |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Base URL is configurable via the `SOLVELA_GATEWAY_URL` runtime setting (read via `runtime.getSetting()`, not `process.env`) with a sensible localhost default.
- Provider must be stateless across invocations; no in-memory caching of signed payments.
- Keep the return value a plain string suited for prompt injection — ElizaOS providers contribute context, not callable APIs.

### Testing Requirements
```bash
cd integrations/elizaos && npm run build
```
There is no test suite yet — `npm test` prints a placeholder.

### Common Patterns
- Use `fetch` (Node 20+); avoid heavy axios/superagent. Failures are swallowed into a descriptive status string so probe errors never crash the agent.

## Dependencies

### Internal
_(none — provider is a leaf module)_

### External
- `@elizaos/core`.

<!-- MANUAL: -->
