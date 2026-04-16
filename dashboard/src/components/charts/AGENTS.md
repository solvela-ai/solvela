<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# charts

## Purpose
Recharts wrappers tuned to the terminal-card design system. Each file exports a single visualization component. Data reshaping is the caller's responsibility — these components take already-shaped series.

## Key Files
| File | Description |
|------|-------------|
| `spend-chart.tsx` | Line/area chart of spend over time |
| `requests-bar.tsx` | Bar chart of requests per time bucket |
| `model-pie.tsx` | Donut/pie chart of model usage distribution |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Each chart is a client component (`"use client"`).
- Keep the public prop interface stable — the consuming pages rely on it.
- Pull palette + font tokens from Tailwind / `globals.css` — don't hardcode hex values inside chart files.
- Support both dark and light themes via `next-themes` at the page level; charts should read computed CSS variables rather than hardcoding colours.

### Testing Requirements
```bash
npm --prefix dashboard test
npm --prefix dashboard run dev  # visual regression check
```

### Common Patterns
- Named export matching the file name (e.g., `SpendChart`).
- Accept a `data` prop + optional display options.

## Dependencies

### Internal
- `@/lib/utils`, `@/lib/theme-config`.

### External
- `recharts`.

<!-- MANUAL: -->
