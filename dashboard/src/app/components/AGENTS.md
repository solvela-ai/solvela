<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# components

## Purpose
Route-scoped components that belong to `app/` — typically MDX helpers or docs-site chrome that aren't reusable across the broader dashboard.

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `docs/` | MDX helpers / components used inside Fumadocs docs pages |

## For AI Agents

### Working In This Directory
- If a component becomes reusable outside this route tree, promote it to `src/components/`.
- Keep MDX-only helpers here — they shouldn't leak into the dashboard's main UI.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- Thin wrappers over base UI primitives, customized for prose contexts (e.g., callouts, code blocks with copy).

## Dependencies

### Internal
- `@/components/ui` for base primitives.

### External
- Fumadocs, MDX.

<!-- MANUAL: -->
