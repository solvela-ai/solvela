<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# dashboard

## Purpose
The product web app at `solvela.vercel.app`. A Next.js 16 App Router project that combines the Solvela dashboard (Overview / Usage / Models / Wallet / Settings) with the public docs site powered by Fumadocs. Styled with Tailwind + a serif-typography, terminal-card design system.

## Key Files
| File | Description |
|------|-------------|
| `README.md` | Local setup + deploy notes |
| `package.json` | NPM manifest — Next.js, Fumadocs, Tailwind, Recharts, Vitest |
| `next.config.mjs` | Next.js config (experimental features, MDX) |
| `source.config.ts` | Fumadocs content-source config (maps `content/docs/` → route) |
| `postcss.config.mjs` | PostCSS / Tailwind config |
| `eslint.config.mjs` | ESLint flat config |
| `tsconfig.json` | TypeScript compiler config |
| `vitest.config.ts` | Vitest config |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | All application code (see `src/AGENTS.md`) |
| `content/` | Fumadocs MDX content for the docs site (see `content/AGENTS.md`) |
| `public/` | Static assets served at `/` (see `public/AGENTS.md`) |
| `node_modules/` | Installed deps (not checked in) |

## For AI Agents

### Working In This Directory
- App Router only — no `pages/` directory.
- Fumadocs owns `/docs`; the rest of the routes are the product dashboard.
- Design system: serif display font, terminal-card component, restrained palette. Don't fight it.
- Before claiming UI work is done, run the dev server and exercise the feature in a browser (CLAUDE.md golden rule).

### Testing Requirements
```bash
npm --prefix dashboard install          # one-time
npm --prefix dashboard run dev          # local dev
npm --prefix dashboard test             # vitest unit tests
npm --prefix dashboard run lint
npm --prefix dashboard run build
```

### Common Patterns
- Server Components by default; reach for `"use client"` only when truly needed.
- Tailwind utility classes; shared tokens in Tailwind config / `globals.css`.
- Data fetching via `src/lib/api.ts` (delegates to the gateway).

## Dependencies

### Internal
- Solvela gateway HTTP (for live data, read-only).

### External
- Next.js 16, Fumadocs, Tailwind, Recharts, Vitest.

<!-- MANUAL: -->
