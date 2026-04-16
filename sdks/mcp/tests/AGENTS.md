<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
Tests for the MCP server — tool registration, input-schema validation, happy path, 402 retry, error mapping.

## Key Files
| File | Description |
|------|-------------|
| `server.test.ts` | Boots the server in-process, drives it through the MCP transport, asserts tool outcomes |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Boot the server with an in-memory transport rather than stdio for deterministic tests.
- Mock the Solvela HTTP client so tool tests don't depend on a running gateway.
- Every tool needs at least: valid input, invalid input (schema rejection), 402 handled, upstream error surfaced.

### Testing Requirements
```bash
npm --prefix sdks/mcp test
```

### Common Patterns
- Fixture a `Client` mock shared across tool tests.
- Assert both response payload and any side effects (e.g., logs, metrics) where relevant.

## Dependencies

### Internal
- `../src/` — server under test.

### External
- Test runner + MCP SDK test utilities from `../package.json`.

<!-- MANUAL: -->
