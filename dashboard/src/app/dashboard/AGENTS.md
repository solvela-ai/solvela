<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# dashboard

## Purpose
The authenticated product dashboard. One subdirectory per top-level page: Overview, Usage, Models, Wallet, Settings. Shared `layout.tsx` wraps all child routes with the sidebar + topbar shell.

## Key Files
| File | Description |
|------|-------------|
| `layout.tsx` | Shell layout — renders `Shell`/`Sidebar`/`Topbar` around `{children}` |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `overview/` | `/dashboard/overview` — stats cards + spend/requests/model-mix charts |
| `usage/` | `/dashboard/usage` — detailed usage table + filters |
| `models/` | `/dashboard/models` — model catalogue + per-model pricing |
| `wallet/` | `/dashboard/wallet` — wallet connect, balance, recent payments |
| `settings/` | `/dashboard/settings` — preferences, API keys, org selection |

## For AI Agents

### Working In This Directory
- Pages are Server Components fetching data through `@/lib/api.ts` + mock data in dev (`@/lib/mock-data.ts`).
- Wallet page (`wallet/wallet-client.tsx`) is a client component because it needs browser wallet APIs.
- Before claiming a change done, run `npm --prefix dashboard run dev` and visit the affected route.

### Testing Requirements
```bash
npm --prefix dashboard test
npm --prefix dashboard run dev  # visual verification
```

### Common Patterns
- Compose layout with `Shell`, `Sidebar`, `Topbar` from `@/components/layout`.
- Stat cards + charts from `@/components/ui` and `@/components/charts`.
- Data fetching in the server component; pass serializable data to client components.

## Dependencies

### Internal
- `@/components/layout`, `@/components/ui`, `@/components/charts`, `@/lib/api`, `@/lib/mock-data`.

### External
- Next.js 16, Recharts, Tailwind.

<!-- MANUAL: -->
