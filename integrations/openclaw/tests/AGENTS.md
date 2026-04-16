<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
Unit tests for the OpenClaw integration.

## Key Files
| File | Description |
|------|-------------|
| `plugin.test.ts` | Verifies plugin registration, 402 handling, and request forwarding to the configured gateway URL |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Mock HTTP with a lightweight library (e.g., `undici.MockAgent` or `nock`) — never hit a real gateway in unit tests.
- When adding a new request path, add both a 200 case and a 402→sign→200 case.

### Testing Requirements
```bash
npm --prefix integrations/openclaw test
```

### Common Patterns
- Assert both the final response and the sequence of HTTP calls (e.g., 402 first, then the retry with the signed header).

## Dependencies

### Internal
- `../src/` — the integration code under test.

### External
- Whatever test runner is pinned in `../package.json`.

<!-- MANUAL: -->
