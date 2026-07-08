<!-- Generated: 2026-05-27 | Files scanned: 32 | Token estimate: ~700 -->

# Frontend Codemap (Next.js 16 + Tailwind)

**Last Updated:** 2026-05-27  
**Entry:** `dashboard/src/app/layout.tsx`  
**Framework:** Next.js 16, TypeScript, Tailwind CSS, Recharts

## Page Tree

```
src/app/
├── layout.tsx                          (root layout, theme provider)
├── page.tsx                            (landing page)
├── sitemap.ts                          (dynamic sitemap)
│
├── api/
│   └── search/
│       └── route.ts                    (search API endpoint)
│
├── dashboard/
│   ├── layout.tsx                      (dashboard layout, sidebar nav)
│   ├── page.tsx                        (dashboard root)
│   ├── overview/page.tsx               (overview: recent requests)
│   ├── usage/page.tsx                  (spend analytics, trends)
│   ├── models/page.tsx                 (model catalog, pricing)
│   ├── wallet/page.tsx                 (wallet balances, transaction history)
│   └── settings/page.tsx               (org/team/api key management)
│
├── metrics/page.tsx                    (public metrics dashboard)
├── sponsor/page.tsx                    (sponsors/backers page)
│
└── docs/
    ├── layout.tsx                      (docs layout with sidebar)
    └── [[...slug]]/page.tsx            (Fumadocs catch-all route)
```

## Core Components

### Layout & Navigation
- **layout.tsx** — Root layout with Theme Provider, TraceProvider
- **dashboard/layout.tsx** — Sidebar navigation, breadcrumbs
- **theme-provider.tsx** — Tailwind + dark mode toggle

### Shared Components
- **lib/layout.shared.tsx** — Reusable layout blocks (Header, Footer, Sidebar)
- **lib/icons.tsx** — Icon library (Lucide + custom SVG)
- **components/mdx.tsx** — MDX renderer for docs

## Pages Detail

### Landing Page (src/app/page.tsx)
- Hero section (copy, CTA)
- Feature blocks
- Pricing overview
- Testimonials
- CTA footer

### Dashboard Overview (src/app/dashboard/page.tsx)
- **Wallet Status** — Connected wallet, USDC balance, recent activity
- **Quick Stats** — Total spend this month, requests today, top model
- **Recent Requests** — Table of last 10 chat completions
- **Alerts** — Budget warnings, low balance

### Usage Analytics (src/app/dashboard/usage/page.tsx)
- **Time Series Charts** — Spend/requests over time (daily, hourly)
- **Model Breakdown** — Pie chart of usage by model
- **Provider Breakdown** — Pie chart of usage by provider
- **Cost Trending** — Line chart of cost/request
- **Export** — Download CSV of usage data

Uses **Recharts** for charting.

### Models Catalog (src/app/dashboard/models/page.tsx)
- **Model Grid/Table** — All available models with:
  - Name, provider, context window
  - Input/output pricing (per-token)
  - Capabilities (streaming, vision, tools, reasoning)
  - Try Now button
- **Filters** — By provider, capability, reasoning tier
- **Sorting** — By cost, speed, rating

### Wallet (src/app/dashboard/wallet/page.tsx)
- **Balance Display** — Connected wallet USDC balance
- **Transaction History** — List of pay-in/pay-out transactions
- **Deposit Widget** — Instructions to deposit USDC
- **Withdrawal** — Claim unused balance

### Settings (src/app/dashboard/settings/page.tsx)
- **Org Settings** — Name, slug, members, teams
- **Team Management** — Create/edit teams, assign wallets
- **API Keys** — Generate, revoke, list keys with expiration
- **Budget Management** — Set daily/monthly limits per wallet
- **Audit Log** — View action history (member added, budget changed, etc.)
- **Billing/Usage** — Month-to-date spend, usage trends

### Public Metrics (src/app/metrics/page.tsx)
- **System Stats** — Uptime, requests/sec, avg latency
- **Network Charts** — Real-time request flow
- **Provider Health** — Status of each LLM provider
- **Model Rankings** — Most used models

### Docs (src/app/docs/layout.tsx + [...slug]/page.tsx)
- **Sidebar Navigation** — Auto-generated from `content/` markdown files
- **Content Renderer** — MDX → React via Source
- **TOC** — Auto table-of-contents from headings
- **Breadcrumbs** — Navigation path display

## Utilities

### lib/api.ts
Flat collection of async helpers calling the gateway directly (no nested `api` object). Each helper is an exported `async function`.
```typescript
// Public read endpoints
export async function fetchHealth(): Promise<HealthResponse>;
export async function fetchPricing(): Promise<PricingResponse>;
export async function fetchModels(): Promise<{ data: PricingResponse["models"] }>;
export async function fetchServices(): Promise<ServicesResponse | null>;
export async function fetchEscrowConfig(): Promise<EscrowConfig | null>;
export async function fetchEscrowHealth(): Promise<EscrowHealth | null>;
export async function fetchPublicMetrics(): Promise<PublicMetricsResponse>;

// Admin (require Authorization: Bearer <admin-token>)
export async function fetchAdminStats(token: string): Promise<AdminStatsResponse>;

// Org-scoped (require Authorization: Bearer <solvela_k_...>)
export async function fetchOrgs(): Promise<ApiResult<OrgEntry[]>>;
export async function fetchTeams(orgId: string): Promise<ApiResult<TeamEntry[]>>;
export async function fetchMembers(orgId: string): Promise<ApiResult<MemberEntry[]>>;
export async function fetchApiKeys(orgId: string): Promise<ApiResult<ApiKeyEntry[]>>;
export async function createApiKey(...): Promise<ApiResult<CreateApiKeyResponse>>;
export async function revokeApiKey(...): Promise<ApiResult<void>>;
export async function fetchAuditLogs(orgId: string): Promise<ApiResult<AuditLogEntry[]>>;
export async function fetchOrgStats(orgId: string): Promise<ApiResult<OrgStats>>;
export async function createTeam(orgId: string, name: string): Promise<ApiResult<TeamEntry>>;
export async function setTeamBudget(...): Promise<ApiResult<TeamBudget>>;
export async function fetchTeamBudget(...): Promise<ApiResult<TeamBudget>>;
```
The dashboard does not call `/v1/chat/completions` directly from the client — chat is exercised through the gateway only.

### lib/auth.ts
Thin `localStorage`-backed API-key helpers. The dashboard authenticates via API key, not a Solana wallet adapter.
```typescript
export function getApiKey(): string | null;
export function setApiKey(key: string): void;
export function clearApiKey(): void;
export function hasApiKey(): boolean;
```

### lib/mock-data.ts
Mock data for dev when API unavailable.
```typescript
export const mockModels = [
  { id: "gpt-4o", name: "GPT-4 Omni", provider: "openai", ... },
  // ...
];
```

### lib/metrics-aggregator.ts
Aggregates raw usage data into time-series.
```typescript
export const aggregateUsage = (logs: SpendLog[]) => ({
  daily: Map<Date, number>,      // total_spend by day
  hourly: Map<Date, number>,     // total_spend by hour
  byModel: Map<string, number>,  // total_spend by model
  byProvider: Map<string, number>,  // total_spend by provider
});
```

### lib/utils.ts
Helper functions (format currency, date, truncate address, etc.).

### lib/theme-config.ts
Design tokens.
```typescript
export const theme = {
  colors: { primary: "#...", accent: "#...", ... },
  spacing: { xs: "0.25rem", sm: "0.5rem", ... },
  typography: { base: "1rem", lg: "1.25rem", ... },
};
```

### lib/diagram-colors.ts
Chart colors for consistent visualization.

## Styling

- **Tailwind CSS** — Utility-first CSS framework
- **CSS Modules** — Component-scoped styles (optional)
- **Dark Mode** — Next.js 16 dark mode support

Responsive breakpoints: `sm` (640px), `md` (768px), `lg` (1024px), `xl` (1280px).

## Forms & Input

- **React Hook Form** — Form state management (if used)
- **Validation** — Zod or custom validators
- **Error Display** — Inline field errors + toast notifications

## State Management

- **React Context** — For theme, auth, org context
- **TanStack Query** (if used) — Server state (API data caching)
- **useState/useReducer** — Local component state

## API Integration

The dashboard calls the gateway over HTTP directly from client components using `lib/api.ts` helpers; the gateway base URL comes from `NEXT_PUBLIC_GATEWAY_URL`. A server-side helper at `src/proxy.ts` exists for cases where the request must be forwarded with a server-only token.

The literal route `/api/proxy/*` does **not** exist — the only server route under `app/api/` is `app/api/search/route.ts` (the Fumadocs search endpoint).

## Testing

- **Vitest** — Unit tests for utils and `lib/` helpers
- **Config:** `vitest.config.ts`
- **Test files:** `dashboard/src/__tests__/*.test.ts` (currently `mock-data.test.ts`, `api.test.ts`, `utils.test.ts`, `metrics-aggregator.test.ts`)

Run with `npm --prefix dashboard test`. The dashboard does not currently bundle React component tests or an end-to-end Playwright suite — there is no `e2e` npm script in `dashboard/package.json`.

## Build & Deployment

- **Build** — `npm run build` (Next.js compilation)
- **Start** — `npm start` (production server)
- **Dev** — `npm run dev` (dev server with hot reload)
- **Deploy** — Vercel (automatic from git push)
- **Hosting:** `app.solvela.ai` (aliases: `solvela.ai`, `docs.solvela.ai`)

## Environment Variables (frontend)

| Variable | Purpose | Example |
|----------|---------|---------|
| `NEXT_PUBLIC_GATEWAY_URL` | Gateway API base URL | http://localhost:8402 or https://gateway.solvela.ai |
| `NEXT_PUBLIC_SOLANA_RPC` | Solana RPC endpoint | https://api.mainnet-beta.solana.com |
| `NEXT_PUBLIC_APP_NAME` | App display name | Solvela |
| `GATEWAY_API_KEY` | Server-side gateway auth (for proxy) | secret-key (not exposed to client) |

## Layout Patterns

### Sidebar Layout (Dashboard)
```
┌──────────────────────────────┐
│ Header (logo, user menu)     │
├──────┬──────────────────────┤
│      │                      │
│ Side │   Main Content       │
│ bar  │                      │
│      │                      │
└──────┴──────────────────────┘
```

### Card Layout (Metrics)
```
┌─────────┬─────────┬─────────┐
│  Stat 1 │  Stat 2 │  Stat 3 │
├─────────┴─────────┴─────────┤
│    Large Chart Area         │
├─────────┬─────────┬─────────┤
│ Detail  │ Detail  │ Detail  │
└─────────┴─────────┴─────────┘
```

## Component Examples

### Model Card
```typescript
export function ModelCard({ model }: { model: Model }) {
  return (
    <div className="border rounded-lg p-4">
      <h3>{model.display_name}</h3>
      <p className="text-sm text-gray-600">{model.provider}</p>
      <p className="text-xs">
        {model.input_cost_per_million}¢ / 1M input tokens
      </p>
      <div className="flex gap-2 mt-2">
        {model.supports_streaming && <Badge>Streaming</Badge>}
        {model.supports_vision && <Badge>Vision</Badge>}
      </div>
      <button onClick={() => tryModel(model.model_id)}>Try Now</button>
    </div>
  );
}
```

### Usage Chart
```typescript
export function UsageChart({ data }: { data: UsageData[] }) {
  return (
    <LineChart data={data} width={600} height={300}>
      <CartesianGrid strokeDasharray="3 3" />
      <XAxis dataKey="date" />
      <YAxis />
      <Tooltip />
      <Legend />
      <Line 
        type="monotone" 
        dataKey="spend_usdc" 
        stroke="#8884d8" 
        name="Spend (USDC)" 
      />
    </LineChart>
  );
}
```

## Related

- [Architecture Overview](architecture.md)
- [Backend Routes](backend.md)
- [Database Schema](data.md)
