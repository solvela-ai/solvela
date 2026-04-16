<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# __tests__

## Purpose
Vitest unit tests for the dashboard. Covers `@/lib/api`, mock data shape, and utility helpers.

## Key Files
| File | Description |
|------|-------------|
| `api.test.ts` | Tests for `@/lib/api` — request shape, error mapping, response parsing |
| `mock-data.test.ts` | Ensures mock data remains well-shaped so UI doesn't silently break |
| `utils.test.ts` | Tests for helpers in `@/lib/utils` |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- When you add a helper or API call, add a matching test here.
- Mock HTTP via `vi.fn()` on `globalThis.fetch` or with MSW if already pinned in `package.json`.
- Run tests locally before pushing — CI gates on them.

### Testing Requirements
```bash
npm --prefix dashboard test
npm --prefix dashboard test -- api
```

### Common Patterns
- `describe` / `it` / `expect` — Vitest globals enabled via `vitest.config.ts`.
- Arrange / Act / Assert structure.

## Dependencies

### Internal
- `@/lib/*` — code under test.

### External
- `vitest`.

<!-- MANUAL: -->
