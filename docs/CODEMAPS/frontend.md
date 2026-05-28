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
│   ├── page.tsx                        (overview: wallet, recent requests)
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
    └── [...slug]/page.tsx              (markdown docs (via Source))
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
HTTP client wrapper for gateway API.
```typescript
export const api = {
  chat: {
    completions(req: ChatRequest): Promise<ChatResponse>,
    stream(req: ChatRequest): ReadableStream,
  },
  models: {
    list(): Promise<Model[]>,
    get(id: string): Promise<Model>,
  },
  orgs: {
    get(id: string): Promise<Org>,
    update(id, data),
    members: { list(), add(), remove() },
    teams: { list(), create() },
    apiKeys: { list(), create(), revoke() },
    budgets: { list(), set() },
  },
  wallet: {
    balance(): Promise<Decimal>,
    history(): Promise<Transaction[]>,
  },
};
```

### lib/auth.ts
Wallet connection + session management.
```typescript
export const useAuth = () => {
  const wallet = useWallet();  // Solana wallet adapter
  const [session, setSession] = useState(null);
  // ... connect, sign message, verify signature
};
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

**Proxy-based access:**
- Dashboard sends requests to `/api/proxy/*`
- Proxy forwards to `http://localhost:8402/v1/*`
- Handles auth headers, request/response transformation

See `src/proxy.ts`:
```typescript
export default async function handler(req, res) {
  const { path, method, body } = req;
  const response = await fetch(`http://localhost:8402${path}`, {
    method,
    headers: { "Authorization": `Bearer ${process.env.GATEWAY_API_KEY}` },
    body,
  });
  return res.json(response.json());
}
```

## Testing

- **Vitest** — Unit tests for utils, hooks
- **React Testing Library** — Component tests
- **Config:** `vitest.config.ts`
- **Test files:** `src/__tests__/*.test.ts` (or `.tsx`)

```typescript
import { render, screen } from '@testing-library/react';
import { Dashboard } from './dashboard/page';

test('displays welcome message', () => {
  render(<Dashboard />);
  expect(screen.getByText(/welcome/i)).toBeInTheDocument();
});
```

## Build & Deployment

- **Build** — `npm run build` (Next.js compilation)
- **Start** — `npm start` (production server)
- **Dev** — `npm run dev` (dev server with hot reload)
- **Deploy** — Vercel (automatic from git push)
- **Hosting:** `solvela.vercel.app`

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
