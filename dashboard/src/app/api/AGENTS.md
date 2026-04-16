<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# api

## Purpose
Next.js Route Handlers — server-side endpoints exposed by the dashboard. Currently backs Fumadocs' client-side search.

## Key Files
_(no loose files)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `search/` | `GET /api/search` — Fumadocs search index endpoint |

## For AI Agents

### Working In This Directory
- Route handlers are server-only — never import client-only modules here.
- Do **not** expose admin / write operations through the dashboard without a separate auth layer; the dashboard is a read-only client of the gateway for now.
- If you add a new endpoint, add a corresponding Vitest test under `../../__tests__/`.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- `export async function GET(request: Request)` signature for GET handlers.
- Return `Response.json(data)` or `new Response(body, { headers })`.

## Dependencies

### Internal
- `@/lib/source` for the Fumadocs search index.

### External
- Next.js Route Handlers, Fumadocs search adapter.

<!-- MANUAL: -->
