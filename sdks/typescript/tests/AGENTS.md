<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
Unit tests for the TypeScript SDK.

## Key Files
| File | Description |
|------|-------------|
| `client.test.ts` | Client behaviour — 200 path, 402→sign→retry, error mapping, streaming |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Mock HTTP (`undici.MockAgent`, `nock`, or the runner's built-in mock) — never hit a real gateway.
- For signing tests, use deterministic seeds (`Uint8Array.from([…])`) so signatures are stable.
- Each public method gets: happy path, error path, edge-case (e.g., empty messages).

### Testing Requirements
```bash
npm --prefix sdks/typescript test
```

### Common Patterns
- Arrange / act / assert blocks separated visually.
- Prefer the runner pinned in `../package.json`.

## Dependencies

### Internal
- `../src/` — code under test.

### External
- Test runner + HTTP mocking library from `../package.json`.

<!-- MANUAL: -->
