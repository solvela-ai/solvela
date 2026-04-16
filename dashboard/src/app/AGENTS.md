<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# app

## Purpose
Next.js App Router entry. `layout.tsx` is the root layout (html/body, font, providers); `page.tsx` is the marketing landing page. Product routes live under `dashboard/`; docs live under `docs/[[...slug]]`; JSON search under `api/search/`.

## Key Files
| File | Description |
|------|-------------|
| `layout.tsx` | Root layout — global fonts, theme provider, tailwind baseline |
| `page.tsx` | Marketing landing page |
| `globals.css` | Tailwind directives + global CSS variables (design tokens) |
| `favicon.ico` | Tab icon |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `api/` | Route handlers — currently Fumadocs search endpoint (see `api/AGENTS.md`) |
| `dashboard/` | Authenticated product routes — Overview / Usage / Models / Wallet / Settings (see `dashboard/AGENTS.md`) |
| `docs/` | Fumadocs-backed docs site at `/docs/*` (see `docs/AGENTS.md`) |
| `providers/` | Client-side React providers (theme, etc.) (see `providers/AGENTS.md`) |
| `components/` | Route-scoped components (e.g., docs MDX helpers) (see `components/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Root layout is shared — changes here affect every route. Touch with care.
- Marketing `page.tsx` is separate from the product dashboard; don't put product-auth logic here.
- Edit design tokens in `globals.css`; don't redefine them per-route.

### Testing Requirements
```bash
npm --prefix dashboard run dev
# Open http://localhost:3000 and exercise the route you changed
npm --prefix dashboard test
```

### Common Patterns
- Server Components by default; client components are opt-in with `"use client"` at the top.
- File-based routing — `page.tsx` = route, `layout.tsx` = shared shell, `loading.tsx` = suspense boundary.

## Dependencies

### Internal
- `@/components`, `@/lib/*`.

### External
- Next.js 16, React 19, Fumadocs, Tailwind.

<!-- MANUAL: -->
