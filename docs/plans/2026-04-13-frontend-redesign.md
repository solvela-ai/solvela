# Solvela Frontend Redesign — Engineering Prompt

> **Date**: 2026-04-13
> **Reference**: Anthropic API Docs (platform.claude.com/docs), Mintlify 2025 API Docs Report, Fumadocs/Unmint
> **Scope**: Full dashboard → docs-first site redesign with dark mode default
> **Engine**: Fumadocs (fumadocs-mdx + fumadocs-ui + fumadocs-core) on existing Next.js 16 app

---

## Vision

Redesign the Solvela dashboard (`dashboard/`) from a standalone analytics dashboard into a **documentation-first developer platform** modeled after Anthropic's API docs site. The site serves two audiences:

1. **Developers integrating with the gateway** — need docs, API reference, quickstart guides
2. **Operators running a gateway** — need analytics, wallet management, settings

Dark mode is the **default and only theme** (no light mode toggle). The existing orange brand palette carries forward.

**Key architectural decision**: Use **Fumadocs** as the documentation engine. It provides the three-column layout, sidebar navigation, search, MDX content pipeline, code blocks with syntax highlighting, TOC, callouts, tabs, steps, and OpenAPI integration — all out of the box. The custom dashboard pages coexist alongside Fumadocs routes in the same Next.js app.

---

## Why Fumadocs (Not Hand-Rolled)

| Concern | Hand-rolled (old plan) | Fumadocs |
|---------|------------------------|----------|
| Doc components (Callout, CodeBlock, Steps, Tabs, Cards) | Build ~8 components from scratch | Built-in via `fumadocs-ui/mdx` |
| Three-column layout (sidebar + content + right TOC) | Build from scratch | `DocsLayout` + `DocsPage` |
| Sidebar navigation with collapsible sections | Build from scratch | Auto-generated from `meta.json` page tree |
| Full-text search | "Placeholder" | Built-in Orama search (zero config) |
| MDX content pipeline | "Future phase" | Day 1 via `fumadocs-mdx` |
| Syntax highlighting | Integrate Shiki manually | Built-in Shiki integration |
| Dark mode | Manual CSS token system | Built-in with `next-themes` |
| OpenAPI API reference pages | Build ParamTable, ResponseBlock, Endpoint | `fumadocs-openapi` generates from spec |
| Table of contents (right side) | Build IntersectionObserver + TOC component | Auto-extracted from MDX headings |
| Previous/Next page nav | Build from navigation tree | Built-in in `DocsPage` |
| Time to ship docs section | Weeks | Days |
| Dashboard pages | Custom React pages | Custom React pages (unchanged) |

---

## Design Reference: Anthropic API Docs

### Layout Architecture

Fumadocs provides this layout natively via `DocsLayout`:

```
┌──────────────────────────────────────────────────────┐
│  Topbar: Logo · Nav tabs · Search (⌘K) · GitHub      │
├────────┬─────────────────────────────┬───────────────┤
│        │                             │ On This Page  │
│  Left  │     Main Content Area       │ (right TOC)   │
│  Side  │     max-width: ~768px       │ sticky, auto  │
│  bar   │     centered in column      │ highlights    │
│  w-60  │                             │ current sect  │
│        │                             │               │
│  auto  │  MDX Content:               │ - Section 1   │
│  from  │  Headings, prose            │ - Section 2   │
│  page  │  Code blocks (tabbed)       │   - Sub 2.1   │
│  tree  │  Callouts (Note/Warn/Tip)   │ - Section 3   │
│        │  Tables, Cards, Steps       │               │
│        │                             │               │
├────────┴─────────────────────────────┴───────────────┤
│  Footer: Prev/Next page (auto) · Links               │
└──────────────────────────────────────────────────────┘
```

Dashboard pages use a **separate layout** — no Fumadocs wrapping, just sidebar + full-width content (existing Shell component, dark-themed).

---

## Installation & Integration

### Packages to Install

```bash
cd dashboard
npm i fumadocs-mdx fumadocs-core fumadocs-ui @types/mdx
# For OpenAPI reference pages (Phase 3):
npm i fumadocs-openapi shiki
```

### Configuration Files

#### `dashboard/source.config.ts`
```ts
import { defineDocs, defineConfig } from 'fumadocs-mdx/config';

export const docs = defineDocs({
  dir: 'content/docs',
});

export default defineConfig();
```

#### `dashboard/next.config.mjs` (replace next.config.ts)
```js
import { createMDX } from 'fumadocs-mdx/next';

const config = {
  reactStrictMode: true,
};

const withMDX = createMDX();

export default withMDX(config);
```

#### `dashboard/tsconfig.json` — add path alias
```json
{
  "compilerOptions": {
    "paths": {
      "@/*": ["./src/*"],
      "collections/*": ["./.source/*"]
    }
  }
}
```

#### `dashboard/src/lib/source.ts`
```ts
import { docs } from 'collections/server';
import { loader } from 'fumadocs-core/source';

export const source = loader({
  baseUrl: '/docs',
  source: docs.toFumadocsSource(),
});
```

#### `dashboard/src/components/mdx.tsx`
```tsx
import defaultMdxComponents from 'fumadocs-ui/mdx';
import type { MDXComponents } from 'mdx/types';

export function getMDXComponents(components?: MDXComponents) {
  return {
    ...defaultMdxComponents,
    ...components,
  } satisfies MDXComponents;
}

export const useMDXComponents = getMDXComponents;

declare global {
  type MDXProvidedComponents = ReturnType<typeof getMDXComponents>;
}
```

#### `dashboard/src/lib/layout.shared.tsx`
```tsx
import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <span className="flex items-center gap-2">
          <span className="flex h-7 w-7 items-center justify-center rounded-lg bg-orange-500 text-white text-xs font-bold">S</span>
          <span className="font-semibold">Solvela</span>
        </span>
      ),
    },
    links: [
      { text: 'Dashboard', url: '/dashboard/overview' },
      { text: 'GitHub', url: 'https://github.com/your-org/solvela', external: true },
    ],
  };
}
```

---

## Color System (Dark Mode Default)

### Approach: Override Fumadocs CSS Variables

Fumadocs uses `--color-fd-*` CSS variables (Shadcn-inspired). Override them in `globals.css` to enforce dark-only with the Solvela orange palette.

#### `dashboard/src/app/globals.css`
```css
@import 'tailwindcss';
@import 'fumadocs-ui/css/neutral.css';
@import 'fumadocs-ui/css/preset.css';

/*
 * Solvela dark theme — override Fumadocs defaults.
 * Dark mode is the ONLY mode. No .dark selector needed.
 * Apply dark values to :root directly.
 */
:root {
  color-scheme: dark;

  /* Fumadocs core variables — dark values */
  --color-fd-background: #0a0a0a;
  --color-fd-foreground: #f5f5f5;
  --color-fd-card: #141414;
  --color-fd-card-foreground: #f5f5f5;
  --color-fd-popover: #1c1c1c;
  --color-fd-popover-foreground: #f5f5f5;
  --color-fd-primary: #f97316;            /* orange-500 brand */
  --color-fd-primary-foreground: #ffffff;
  --color-fd-secondary: #1c1c1c;
  --color-fd-secondary-foreground: #a3a3a3;
  --color-fd-muted: #262626;
  --color-fd-muted-foreground: #737373;
  --color-fd-accent: rgba(249, 115, 22, 0.1); /* orange tinted */
  --color-fd-accent-foreground: #fb923c;       /* orange-400 */
  --color-fd-border: #262626;
  --color-fd-ring: #f97316;

  /* Solvela-specific tokens (for dashboard pages) */
  --color-bg-base: #0a0a0a;
  --color-bg-surface: #141414;
  --color-bg-surface-raised: #1c1c1c;
  --color-bg-surface-hover: #262626;
  --color-bg-inset: #0f0f0f;
  --color-border: #262626;
  --color-border-subtle: #1c1c1c;
  --color-border-emphasis: #404040;
  --color-text-primary: #f5f5f5;
  --color-text-secondary: #a3a3a3;
  --color-text-tertiary: #737373;
  --color-brand: #f97316;
  --color-brand-hover: #ea580c;
  --color-brand-subtle: rgba(249, 115, 22, 0.1);
  --color-brand-text: #fb923c;
  --color-success: #22c55e;
  --color-warning: #eab308;
  --color-error: #ef4444;
  --color-info: #3b82f6;

  /* Provider badges */
  --color-provider-openai: #10a37f;
  --color-provider-anthropic: #d97757;
  --color-provider-google: #4285f4;
  --color-provider-xai: #e5e5e5;
  --color-provider-deepseek: #536dfe;
}

/* Force dark class on html to tell Fumadocs/next-themes we're always dark */
html {
  color-scheme: dark;
}

@theme inline {
  --color-bg-base: var(--color-bg-base);
  --color-bg-surface: var(--color-bg-surface);
  --color-bg-surface-raised: var(--color-bg-surface-raised);
  --color-bg-surface-hover: var(--color-bg-surface-hover);
  --color-bg-inset: var(--color-bg-inset);
  --color-border: var(--color-border);
  --color-border-subtle: var(--color-border-subtle);
  --color-border-emphasis: var(--color-border-emphasis);
  --color-text-primary: var(--color-text-primary);
  --color-text-secondary: var(--color-text-secondary);
  --color-text-tertiary: var(--color-text-tertiary);
  --color-brand: var(--color-brand);
  --color-brand-hover: var(--color-brand-hover);
  --color-brand-subtle: var(--color-brand-subtle);
  --color-brand-text: var(--color-brand-text);
  --color-success: var(--color-success);
  --color-warning: var(--color-warning);
  --color-error: var(--color-error);
  --color-info: var(--color-info);
  --font-sans: var(--font-geist-sans);
  --font-mono: var(--font-geist-mono);
}
```

### Typography

- **Font**: Geist Sans (already loaded via `next/font/google`) — remove the hardcoded `Arial` from `globals.css`
- **Mono**: Geist Mono (for code blocks, addresses, API endpoints)
- Fumadocs handles heading scales and prose styling via its built-in Tailwind Typography plugin fork

### Force Dark Mode in RootProvider

In the root layout, configure `next-themes` to force dark:

```tsx
<RootProvider theme={{ defaultTheme: 'dark', forcedTheme: 'dark' }}>
```

This disables the theme toggle and locks to dark mode.

---

## URL Structure & Routing

```
Route                          Source                          Layout
─────────────────────────────────────────────────────────────────────────
/                              app/page.tsx (redirect)         —
/docs                          content/docs/index.mdx          Fumadocs DocsLayout
/docs/quickstart               content/docs/quickstart.mdx     Fumadocs DocsLayout
/docs/concepts/x402            content/docs/concepts/x402.mdx  Fumadocs DocsLayout
/docs/concepts/smart-router    content/docs/concepts/smart-router.mdx
/docs/concepts/escrow          content/docs/concepts/escrow.mdx
/docs/concepts/pricing         content/docs/concepts/pricing.mdx
/docs/sdks/overview            content/docs/sdks/index.mdx
/docs/sdks/typescript          content/docs/sdks/typescript.mdx
/docs/sdks/python              content/docs/sdks/python.mdx
/docs/sdks/go                  content/docs/sdks/go.mdx
/docs/sdks/rust                content/docs/sdks/rust.mdx
/docs/sdks/mcp                 content/docs/sdks/mcp.mdx
/docs/api/overview             content/docs/api/index.mdx
/docs/api/chat-completions     content/docs/api/chat-completions.mdx
/docs/api/models               content/docs/api/models.mdx
/docs/api/pricing              content/docs/api/pricing.mdx
/docs/api/health               content/docs/api/health.mdx
/docs/api/services             content/docs/api/services.mdx
/docs/api/escrow               content/docs/api/escrow.mdx
/docs/api/admin                content/docs/api/admin.mdx
/docs/api/orgs                 content/docs/api/orgs.mdx
/docs/api/errors               content/docs/api/errors.mdx
/docs/changelog                content/docs/changelog.mdx
/dashboard/overview            app/dashboard/overview/page.tsx  Custom dark layout
/dashboard/usage               app/dashboard/usage/page.tsx     Custom dark layout
/dashboard/models              app/dashboard/models/page.tsx    Custom dark layout
/dashboard/wallet              app/dashboard/wallet/page.tsx    Custom dark layout
/dashboard/settings            app/dashboard/settings/page.tsx  Custom dark layout
/api/search                    app/api/search/route.ts          (Fumadocs search API)
```

### File Structure

```
dashboard/
├── source.config.ts                    # Fumadocs MDX collection config
├── next.config.mjs                     # Next.js + Fumadocs MDX plugin
├── content/
│   └── docs/
│       ├── index.mdx                   # /docs → Welcome page
│       ├── meta.json                   # Root navigation ordering
│       ├── quickstart.mdx
│       ├── concepts/
│       │   ├── meta.json               # Section title + page ordering
│       │   ├── x402.mdx
│       │   ├── smart-router.mdx
│       │   ├── escrow.mdx
│       │   └── pricing.mdx
│       ├── sdks/
│       │   ├── meta.json
│       │   ├── index.mdx               # SDK overview
│       │   ├── typescript.mdx
│       │   ├── python.mdx
│       │   ├── go.mdx
│       │   ├── rust.mdx
│       │   └── mcp.mdx
│       ├── api/
│       │   ├── meta.json
│       │   ├── index.mdx               # API reference overview
│       │   ├── chat-completions.mdx
│       │   ├── models.mdx
│       │   ├── pricing.mdx
│       │   ├── health.mdx
│       │   ├── services.mdx
│       │   ├── escrow.mdx
│       │   ├── admin.mdx
│       │   ├── orgs.mdx
│       │   └── errors.mdx
│       └── changelog.mdx
├── src/
│   ├── app/
│   │   ├── layout.tsx                  # Root: RootProvider (forced dark)
│   │   ├── page.tsx                    # / → redirect /docs
│   │   ├── globals.css                 # Fumadocs CSS + Solvela dark overrides
│   │   ├── docs/
│   │   │   ├── layout.tsx              # Fumadocs DocsLayout
│   │   │   └── [[...slug]]/
│   │   │       └── page.tsx            # Fumadocs DocsPage (catch-all)
│   │   ├── dashboard/
│   │   │   ├── layout.tsx              # Custom dashboard layout (Shell + dark tokens)
│   │   │   ├── overview/page.tsx       # Existing (dark-converted)
│   │   │   ├── usage/page.tsx          # Existing (dark-converted)
│   │   │   ├── models/page.tsx         # Existing (dark-converted)
│   │   │   ├── wallet/
│   │   │   │   ├── page.tsx            # Existing (dark-converted)
│   │   │   │   └── wallet-client.tsx
│   │   │   └── settings/page.tsx       # Existing (dark-converted)
│   │   └── api/
│   │       └── search/
│   │           └── route.ts            # Fumadocs search endpoint
│   ├── components/
│   │   ├── mdx.tsx                     # MDX component overrides
│   │   ├── layout/
│   │   │   ├── shell.tsx               # Dashboard mobile shell
│   │   │   ├── sidebar.tsx             # Dashboard sidebar (nav for dashboard pages)
│   │   │   └── topbar.tsx              # Dashboard topbar
│   │   ├── ui/
│   │   │   ├── badge.tsx
│   │   │   ├── stat-card.tsx
│   │   │   ├── status-dot.tsx
│   │   │   ├── toggle.tsx
│   │   │   └── input.tsx
│   │   └── charts/
│   │       ├── spend-chart.tsx
│   │       ├── requests-bar.tsx
│   │       └── model-pie.tsx
│   ├── lib/
│   │   ├── source.ts                   # Fumadocs source loader
│   │   ├── layout.shared.tsx           # Fumadocs layout options (logo, nav links)
│   │   ├── api.ts                      # Gateway API client
│   │   ├── auth.ts                     # API key localStorage
│   │   ├── mock-data.ts
│   │   └── utils.ts
│   └── types/
│       └── index.ts
```

### Navigation via meta.json

Fumadocs auto-generates the sidebar from the file tree + `meta.json` files. No manual `navigation.ts` needed.

#### `content/docs/meta.json`
```json
{
  "title": "Solvela Docs",
  "pages": [
    "---Welcome---",
    "index",
    "quickstart",
    "---Concepts---",
    "concepts",
    "---SDKs---",
    "sdks",
    "---API Reference---",
    "api",
    "---",
    "changelog"
  ]
}
```

#### `content/docs/concepts/meta.json`
```json
{
  "title": "Concepts",
  "pages": ["x402", "smart-router", "escrow", "pricing"]
}
```

#### `content/docs/api/meta.json`
```json
{
  "title": "API Reference",
  "pages": [
    "index",
    "chat-completions",
    "models",
    "pricing",
    "health",
    "services",
    "escrow",
    "admin",
    "orgs",
    "errors"
  ]
}
```

#### `content/docs/sdks/meta.json`
```json
{
  "title": "SDKs",
  "pages": ["index", "typescript", "python", "go", "rust", "mcp"]
}
```

---

## Built-in Fumadocs Components (Available in MDX)

These come from `fumadocs-ui/mdx` — no custom code needed:

| Component | Usage in MDX | What It Does |
|-----------|-------------|--------------|
| `<Callout type="info">` | Notes, warnings, tips | Colored callout boxes with icons |
| `<Tabs>` + `<Tab>` | Language tabs for code examples | Tabbed content with shared state + persistence |
| `<Steps>` + `<Step>` | Quickstart numbered steps | Numbered progression UI |
| `<Cards>` + `<Card>` | Navigation card grids | Linked cards with title + description |
| `<Accordion>` | Collapsible sections | Expandable content |
| `<TypeTable>` | API parameter docs | Type documentation tables |
| `<Files>` | File structure display | Tree view of file structures |
| Code blocks | ` ```ts title="example.ts" ``` ` | Shiki-highlighted code with copy button, title bar |

### MDX Content Example

```mdx
---
title: Welcome to Solvela
description: AI agent payment gateway — USDC-SPL on Solana via x402 protocol
---

import { Cards, Card } from 'fumadocs-ui/mdx';
import { Steps, Step } from 'fumadocs-ui/components/steps';
import { Callout } from 'fumadocs-ui/mdx';
import { Tabs, Tab } from 'fumadocs-ui/components/tabs';

<Callout type="info">
  Solvela is a Solana-native payment gateway for AI agents. No API keys, no
  accounts — just wallets paying with USDC.
</Callout>

## Get Started

<Steps>
<Step>
### Make your first API call

Set up your environment and send a chat completion request.

```bash title="Shell"
curl https://api.solvela.ai/v1/chat/completions \
  -H "Content-Type: application/json" \
  -H "PAYMENT-SIGNATURE: <base64-signed-tx>" \
  -d '{"model": "auto", "messages": [{"role": "user", "content": "Hello"}]}'
```
</Step>

<Step>
### Understand x402 payments

Learn how wallets pay for API calls with USDC-SPL on Solana.

[Read the x402 guide →](/docs/concepts/x402)
</Step>
</Steps>

## Explore

<Cards>
  <Card title="Quickstart" href="/docs/quickstart">
    First API call in 5 minutes
  </Card>
  <Card title="API Reference" href="/docs/api">
    Full endpoint documentation
  </Card>
  <Card title="SDKs" href="/docs/sdks">
    TypeScript, Python, Go, Rust, MCP
  </Card>
  <Card title="Dashboard" href="/dashboard/overview">
    Analytics and wallet management
  </Card>
</Cards>
```

---

## Existing Dashboard Pages — Dark Mode Conversion

The 5 existing dashboard pages keep their current functionality. They live under `/dashboard/*` with their own layout (NOT wrapped in Fumadocs). Apply the dark color tokens from `globals.css`:

| Current Class          | Dark Replacement                    |
|------------------------|-------------------------------------|
| `bg-gray-50`           | `bg-bg-base`                        |
| `bg-white`             | `bg-bg-surface`                     |
| `border-gray-200`      | `border-border`                     |
| `border-gray-100`      | `border-border-subtle`              |
| `text-gray-900`        | `text-text-primary`                 |
| `text-gray-700`        | `text-text-secondary`               |
| `text-gray-500`        | `text-text-secondary`               |
| `text-gray-400`        | `text-text-tertiary`                |
| `bg-gray-50` (hover)   | `bg-bg-surface-hover`               |
| `bg-orange-50`         | `bg-brand-subtle`                   |
| `text-orange-700`      | `text-brand-text`                   |
| `bg-orange-500`        | `bg-brand`                          |
| `hover:bg-orange-600`  | `hover:bg-brand-hover`              |
| `shadow-sm`            | Remove or use very subtle shadow    |
| `bg-amber-50`          | `bg-warning/10`                     |
| `text-amber-800`       | `text-warning`                      |

### Chart colors
- Spend/bar fill: `#f97316` (orange-500) — no change
- Grid lines: use `var(--color-border-subtle)` instead of `#f3f4f6`
- Axis text: use `var(--color-text-tertiary)` instead of `#9ca3af`
- Tooltip bg: `var(--color-bg-surface-raised)`, border: `var(--color-border)`

### Dashboard Layout

```tsx
// app/dashboard/layout.tsx
// Custom layout — NOT Fumadocs. Uses the existing Shell + dark tokens.
import { Shell } from '@/components/layout/shell';

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return <Shell>{children}</Shell>;
}
```

The dashboard sidebar (`components/layout/sidebar.tsx`) renders dashboard-only nav links. It does NOT use Fumadocs page tree — it's the existing custom sidebar, dark-converted.

---

## Future-Proofing (Out of Scope, But Architected For)

These features are NOT implemented in v1 but Fumadocs natively supports upgrading to them:

### 1. AI-Powered Search
- v1: Fumadocs built-in Orama search (works out of the box via `/api/search`)
- Future: Upgrade to Algolia, Typesense, or custom AI search — Fumadocs supports all via pluggable search providers

### 2. Interactive API Playground
- v1: Static API reference pages in MDX
- Future: `fumadocs-openapi` generates interactive pages with "Try it" panels from an OpenAPI spec

### 3. llms.txt / AI Agent Compatibility
- From Mintlify research: "48% of docs traffic is AI agents"
- Future: Add `/llms.txt` route; MDX content is already structured markdown — ideal for LLM consumption
- Fumadocs has built-in LLM integration support

### 4. OpenAPI Auto-Generation
- v1: Hand-written API reference MDX pages
- Future: Point `fumadocs-openapi` at the gateway's OpenAPI spec → auto-generate all API pages
- Setup: `npm i fumadocs-openapi shiki`, create OpenAPI server instance, use `generateFiles()` or virtual sources

### 5. Versioning
- Future: Fumadocs supports multiple doc versions via separate content directories
- Architecture: URL can extend to `/docs/v2/api/...`

### 6. Multi-Language / i18n
- Future: Fumadocs has built-in i18n routing middleware
- Architecture: Adds `/docs/en/...`, `/docs/es/...` locale prefix

### 7. Analytics on Doc Usage
- Future: Standard Next.js analytics + Fumadocs search analytics
- Architecture: Integration point in root layout

### 8. Feedback System
- Future: Fumadocs has built-in feedback collection mechanisms
- Architecture: Can add "Was this helpful?" to each doc page

---

## Implementation Priorities

### Phase 1: Foundation
1. Install Fumadocs packages (`fumadocs-mdx`, `fumadocs-core`, `fumadocs-ui`, `@types/mdx`)
2. Create `source.config.ts`, update `next.config.mjs` (ESM), add tsconfig path
3. Create `lib/source.ts` loader and `lib/layout.shared.tsx` (logo, nav links)
4. Create `components/mdx.tsx` (MDX component overrides)
5. Set up `globals.css` with Fumadocs CSS imports + Solvela dark theme overrides
6. Create `app/docs/layout.tsx` (Fumadocs `DocsLayout`) and `app/docs/[[...slug]]/page.tsx`
7. Create `app/api/search/route.ts` (Fumadocs search API)
8. Update `app/layout.tsx` — wrap in `RootProvider` with forced dark theme
9. Move existing dashboard pages under `app/dashboard/` with their own layout

### Phase 2: Content (MDX Pages)
10. Create `content/docs/` directory with `meta.json` files for navigation
11. Write `index.mdx` — Welcome page (intro, Steps, Cards)
12. Write `quickstart.mdx` — First API call with tabbed code examples (Shell, Python, TS, Go, Rust)
13. Write concept pages: `x402.mdx`, `smart-router.mdx`, `escrow.mdx`, `pricing.mdx`
14. Write API reference pages: `api/index.mdx`, `api/chat-completions.mdx`, `api/models.mdx`, etc.
15. Write SDK pages: `sdks/index.mdx`, `sdks/typescript.mdx`, `sdks/python.mdx`, etc.
16. Write `changelog.mdx`

### Phase 3: Dashboard Dark Conversion
17. Convert all dashboard components to use dark CSS tokens
18. Update chart tooltip/grid colors for dark backgrounds
19. Remove dead wallet page duplication (server page → just renders client component)
20. Test all dashboard pages in dark theme

### Phase 4: Polish
21. Verify search works across all doc pages
22. Responsive testing — mobile sidebar, tables, code blocks
23. Accessibility pass (carry forward audit fixes + verify Fumadocs ARIA)
24. Performance pass (Fumadocs handles code highlighting SSR, verify bundle size)

---

## Technical Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Doc engine | Fumadocs (fumadocs-ui + fumadocs-mdx) | Free, MIT, Next.js native, same stack, all components built-in |
| Content format | MDX with frontmatter from day 1 | Fumadocs requires it; no "migrate later" needed |
| Syntax highlighting | Shiki (via Fumadocs built-in) | SSR-compatible, VS Code themes, zero config |
| Search | Orama (Fumadocs default) | Zero config, full-text, works out of the box |
| Dark mode | `next-themes` via Fumadocs `RootProvider` | `forcedTheme: 'dark'` + CSS variable overrides |
| CSS | Tailwind v4 + Fumadocs CSS presets + custom tokens | Fumadocs handles docs styling; custom tokens for dashboard |
| Dashboard layout | Existing Shell/Sidebar (dark-converted) | Separate from Fumadocs — dashboard is custom React, not MDX |
| API reference (v1) | Hand-written MDX | Ship fast; upgrade to `fumadocs-openapi` later |
| API reference (future) | `fumadocs-openapi` from gateway OpenAPI spec | Auto-generates from spec with interactive playground |
| Font | Geist Sans + Geist Mono | Already loaded; Fumadocs `prose` styling picks it up |
| Chart library | Recharts (dashboard only) | Already in use; unchanged |
| Icons | Lucide React | Already in use; Fumadocs also uses Lucide internally |

---

## What NOT to Do

- **No light mode** — force dark via `RootProvider` `forcedTheme: 'dark'`. No toggle.
- **No custom doc components** — use Fumadocs built-ins (`Callout`, `Tabs`, `Steps`, `Cards`, `TypeTable`). Don't rebuild what's provided.
- **No glassmorphism, gradient text, or decorative gradients** — clean, matte dark surfaces.
- **No bounce/spring animations** — Fumadocs defaults are fine; don't add custom motion.
- **No hero sections or marketing copy** — developer tool, not a landing page.
- **No separate docs site** — docs and dashboard coexist in the same Next.js app.
- **No breadcrumbs** — Fumadocs sidebar + TOC provide enough wayfinding.

---

## Accessibility Requirements

Fumadocs provides strong a11y defaults. Carry forward audit fixes plus:

- Skip-to-content link (already implemented in root layout)
- `role="status"` on StatusDot (already implemented)
- `aria-label` on all icon-only buttons (already implemented)
- Fumadocs sidebar handles `aria-expanded`, `aria-current="page"` automatically
- Fumadocs TOC handles scroll-spy accessibility
- Verify: orange-500 (#f97316) on gray-950 (#0a0a0a) = 4.6:1 contrast ratio (passes WCAG AA)
- Dashboard-specific: `aria-expanded` on collapsible sidebar sections, proper table `<th scope>`
