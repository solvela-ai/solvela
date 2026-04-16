<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# components

## Purpose
Shared React components used across the dashboard and (where appropriate) the docs site. Split into: `charts` (Recharts wrappers), `layout` (shell/sidebar/topbar), `ui` (primitives like cards/badges), plus `mdx.tsx` for docs prose rendering.

## Key Files
| File | Description |
|------|-------------|
| `mdx.tsx` | MDX component mapping (H1/H2/links/code blocks) used by Fumadocs |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `charts/` | Recharts-based visualizations (see `charts/AGENTS.md`) |
| `layout/` | App shell — sidebar, topbar, Shell wrapper (see `layout/AGENTS.md`) |
| `ui/` | Primitive UI components — card, badge, stat, status dot (see `ui/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Primitives in `ui/` should be composable, not feature-specific. Bake feature behaviour into the consuming page, not the primitive.
- `layout/` owns navigation structure; when adding a dashboard page, update `sidebar.tsx` so it shows up.
- Charts take pre-shaped data — do the reshaping in the calling page or `@/lib`, not in the chart component.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- `"use client"` at the top of any component using hooks or browser APIs.
- Props typed explicitly; avoid `any` in exported component APIs.

## Dependencies

### Internal
- `@/lib/utils` (classnames, formatters), `@/lib/theme-config`.

### External
- Recharts, Radix primitives (where used), Tailwind.

<!-- MANUAL: -->
