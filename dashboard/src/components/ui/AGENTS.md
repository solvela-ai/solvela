<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# ui

## Purpose
Primitive UI components shared across pages. Each file is small, reusable, and feature-agnostic. New primitives get added here; feature-specific components live with their feature.

## Key Files
| File | Description |
|------|-------------|
| `terminal-card.tsx` | Signature card component — terminal-style chrome + slot for content |
| `stat-card.tsx` | Stat primitive — label + value + optional delta/trend |
| `badge.tsx` | Small inline label with variant styles |
| `status-dot.tsx` | Coloured status indicator (healthy / degraded / down) |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Primitives should be composable. A stat card that knows it's the "Revenue" card is not a primitive — it goes in a page.
- Props use explicit types, not `React.ComponentProps<'div'>` shortcuts that hide intent.
- Spacing / colour tokens come from Tailwind classes; no inline `style={{…}}` colours.
- `terminal-card` is the visual signature — keep its API consistent; features compose it rather than forking it.

### Testing Requirements
```bash
npm --prefix dashboard test
```

### Common Patterns
- Named exports, one primitive per file.
- Variants via a `variant` prop backed by a tiny `variants` map (or `class-variance-authority` if already in use).

## Dependencies

### Internal
- `@/lib/utils` (`cn(...)` classname combiner).

### External
- Tailwind; Radix primitives (if used).

<!-- MANUAL: -->
