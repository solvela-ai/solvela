<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
All application code for the dashboard + docs site. Routing lives under `app/`; shared UI lives under `components/`; library code and data access live under `lib/`; tests live under `__tests__/`.

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `app/` | Next.js App Router routes (see `app/AGENTS.md`) |
| `components/` | Shared React components (see `components/AGENTS.md`) |
| `lib/` | Helpers, API client, theme config, data mocks (see `lib/AGENTS.md`) |
| `__tests__/` | Vitest unit tests (see `__tests__/AGENTS.md`) |
| `shims/` | Local shims for third-party packages (e.g., `fumadocs-ui`) |
| `test/` | Test setup helpers |
| `types/` | Ambient type declarations |

## For AI Agents

### Working In This Directory
- Keep components pure and colocated with the route that owns them when they're route-specific.
- Cross-route shared UI goes in `components/`. Cross-route data access goes in `lib/api.ts`.
- Default to Server Components; annotate with `"use client"` only where interactivity or browser APIs demand it.
- Ambient types in `types/` — don't leak `any` in exported surfaces.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- Path alias `@/*` → `src/*` (see `tsconfig.json`).
- Tailwind class names; shared design tokens in Tailwind / `globals.css`.

## Dependencies

### Internal
- Solvela gateway HTTP via `lib/api.ts`.

### External
- Next.js, React 19, Tailwind, Recharts, Fumadocs.

<!-- MANUAL: -->
