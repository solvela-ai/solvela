<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# lib

## Purpose
Non-component library code — data access, helpers, content source, theme/icon config, and mock data for dev.

## Key Files
| File | Description |
|------|-------------|
| `api.ts` | HTTP client for the Solvela gateway (server-side fetch wrappers) |
| `auth.ts` | Auth session helpers (client-side) |
| `mock-data.ts` | Deterministic mock datasets for local dev / demos |
| `source.ts` | Fumadocs content source — exposes the `content/docs` tree |
| `utils.ts` | General helpers — `cn()`, formatters, number/time utilities |
| `icons.tsx` | Central icon exports |
| `layout.shared.tsx` | Shared layout fragments used by both dashboard + docs |
| `theme-config.ts` | Colour palette, typography tokens, per-theme overrides |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `shiki/` | Shiki (syntax-highlighter) theme + language presets |

## For AI Agents

### Working In This Directory
- `api.ts` is the single source of truth for gateway calls — don't fetch the gateway directly from a component.
- Keep `utils.ts` small; if a helper grows, split it into its own file.
- `mock-data.ts` is for local dev only — don't import it from production paths.
- Type every exported function.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- Pure functions for formatting/utilities — no side effects.
- Named exports everywhere.

## Dependencies

### Internal
- `@/types` for shared types.

### External
- Fumadocs core (for `source.ts`), Shiki, Tailwind helpers.

<!-- MANUAL: -->
