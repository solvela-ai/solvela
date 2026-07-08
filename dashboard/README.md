# Solvela dashboard

The product web app at [app.solvela.ai](https://app.solvela.ai). A
Next.js 16 App Router project that combines the Solvela dashboard pages
(Overview / Usage / Models / Wallet / Settings) with the public docs site
powered by Fumadocs. Styled with Tailwind and a serif / terminal-card
design system.

For deeper context on the directory layout, design system, and module
conventions, read [`AGENTS.md`](./AGENTS.md) and the nested `AGENTS.md`
files under `src/`.

## Local development

```bash
# One-time install (from repo root or any directory).
npm --prefix dashboard install

# Dev server — opens on http://localhost:3000
npm --prefix dashboard run dev

# Vitest unit tests
npm --prefix dashboard test

# ESLint
npm --prefix dashboard run lint

# Production build
npm --prefix dashboard run build

# Serve the production build locally
npm --prefix dashboard run start
```

Or from inside `dashboard/`:

```bash
cd dashboard
npm install
npm run dev
```

The page entry point lives at `src/app/page.tsx` (the project uses the
`src/` layout, not the bare `app/` layout). Dashboard pages live under
`src/app/dashboard/`; the docs site is under `src/app/docs/` with MDX
content under `content/docs/`.

## Environment variables

All env vars are optional locally — the dashboard falls back to a localhost
gateway and to inert values for missing wallets / admin keys. Production
deployments set these in Vercel.

| Variable | Where read | Description |
|---|---|---|
| `NEXT_PUBLIC_GATEWAY_URL` | `src/lib/api.ts` | Solvela gateway base URL. Defaults to `http://localhost:8402` for local dev. |
| `NEXT_PUBLIC_SITE_URL` | `src/lib/theme-config.ts` | Canonical site URL for OG / Twitter card metadata. Falls back through `VERCEL_PROJECT_PRODUCTION_URL` → `VERCEL_URL` → `http://localhost:3000`. |
| `GATEWAY_ADMIN_KEY` | `src/lib/api.ts` | Server-only admin bearer token used for privileged gateway calls (Overview, Models, and Metrics pages). Never exposed to the browser. |
| `SOLVELA_SOLANA_RECIPIENT_WALLET` | `src/app/dashboard/wallet/page.tsx` | Public recipient wallet shown on the Wallet page. Legacy `RCR_SOLANA_RECIPIENT_WALLET` is also accepted as a fallback. |
| `VERCEL_PROJECT_PRODUCTION_URL`, `VERCEL_URL` | `src/lib/theme-config.ts` | Set automatically by Vercel; consumed for canonical URL detection. |

`next.config.mjs` itself reads no custom env vars — it only declares the
MDX rewrites and Fumadocs config. Env wiring lives in
`src/lib/*` and a small number of server-rendered pages.

## Deployment

Deployed to Vercel from `main`. Production domains (one project): `app.solvela.ai` (dashboard), `docs.solvela.ai` (docs), `solvela.ai` (marketing root).

See the operator notes in [`AGENTS.md`](./AGENTS.md) for design-system
guardrails and the project's "exercise the change in a browser before
claiming done" rule.
