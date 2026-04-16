# HANDOFF.md — Solvela Current State

> **Single source of truth** for project status. See `CLAUDE.md` for how to work in the repo. See `CHANGELOG.md` for history.
> **Last verified:** 2026-04-16 (dashboard ported to docs design system — Source Serif 4, salmon eyebrows, terminal-card pattern)

---

## What Is This

Solvela (formerly RustyClawRouter) is a Solana-native LLM payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via x402. Revenue: 5% fee per call.

Part of the **solvela.ai** ecosystem:

| Product | Purpose | Status |
|---------|---------|--------|
| **Solvela** | LLM payment gateway (this repo) | Deployed on Fly.io |
| **RustyClaw Terminal** | Crypto trading terminal + AI agent (rustyclaw.ai) | Backend deployed, frontend not yet |
| **Telsi.ai** | Multi-tenant AI assistant SaaS | Live on Solvela (migrated from BlockRun 2026-04-07) |

---

## What's Built (verified from codebase)

### Gateway (23 HTTP routes)

| Area | Routes | Status |
|------|--------|--------|
| **Chat completions** | `POST /v1/chat/completions` | Working, 5 providers |
| **Image generation** | `POST /v1/images/generations` | Working |
| **A2A protocol** | `GET /.well-known/agent.json`, `POST /a2a` | Working, x402 payment flow |
| **Models/Services** | `GET /v1/models`, `GET /v1/services`, `POST /v1/services/register`, `POST /v1/services/{id}/proxy` | Working |
| **Escrow** | `GET /v1/escrow/config`, `GET /v1/escrow/health` | Working |
| **Enterprise (orgs)** | 12 endpoints under `/v1/orgs/...` | Working, API key auth |
| **Wallet/Stats** | `GET /v1/wallet/{addr}/stats`, `GET /v1/admin/stats` | Working |
| **Infrastructure** | `GET /health`, `GET /pricing`, `GET /metrics`, `GET /v1/nonce`, `GET /v1/supported` | Working |

### Middleware Stack

Rate limiting, API key extraction (org-scoped), x402 payment extraction, CORS, security headers (CSP, X-Frame-Options, HSTS), request ID tracking, concurrency limiting, global timeout, panic handler.

### Database (7 migrations)

Core tables (wallet_budgets, escrow, claims), escrow claim queue, session tracking, retry scheduling, organizations/teams/members/API keys, audit logs, hourly spend limits.

### Escrow Program (Anchor, standalone)

Trustless USDC-SPL escrow: deposit/claim/refund. PDA vault with timeout refunds. Not a workspace member (dep conflicts). **Deployed to mainnet** (`9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`) with upgrade authority retained by deployer (`B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`).

### SDKs

Python (`sdks/python/`), TypeScript (`sdks/typescript/`), Go (`sdks/go/`), MCP server (`sdks/mcp/`).

### Dashboard & Documentation Site

Next.js 16 + Tailwind v4 + Fumadocs (core + mdx) + Recharts. Redesigned 2026-04-13/14 from standalone dashboard into a docs-first developer platform; design system overhauled 2026-04-16 to match the canonical `rcr-docs-site`.

**Design system (2026-04-16)**: Source Serif 4 added as `--font-serif` (editorial display weight 300/500). Salmon accent `#FE8181` as `--accent-salmon` for eyebrow labels. Terminal-window card pattern (`.terminal-card` / titlebar / screen with 14×14 radial dot pattern) ported from `rcr-docs-site`. All 5 dashboard pages restyled — stat cards now show metric values in serif inside terminal-window screens, charts wrapped in terminal-cards with dot-notation mono titlebars (`spend.usdc.daily`, `system.health`, etc.). Shared globals.css tokens guarantee visual parity with the docs site.

**Docs engine**: Fumadocs-core + fumadocs-mdx for content pipeline (MDX, search, source loading). Custom UI components ported from `rcr-docs-site` — NOT using fumadocs-ui. Custom fonts: DM Sans (body), Archivo (legacy display), JetBrains Mono (data/labels), Source Serif 4 (headings). Custom Shiki syntax themes (solvela-dark/light).

**Docs pages (17 MDX)**: Welcome, Quickstart, Concepts (x402, Smart Router, Escrow, Pricing), API Reference (Overview, Chat Completions, Models, Errors), SDKs (Overview, TypeScript, Python, Go, Rust CLI, MCP).

**Dashboard pages (5)**: Overview, Usage, Models, Wallet, Settings — all wrapped in terminal-window cards. Live under `/dashboard/*` with separate Shell layout. Mock data is convincing (~$247.83 spend, 12.4k requests, realistic curves) — ready for marketing screenshots.

**Route structure**: `/` → redirect to `/docs`, `/docs/*` → Fumadocs MDX pages, `/dashboard/*` → custom dashboard pages, `/api/search` → Fumadocs Orama search.

Deployed to Vercel (`solvela.vercel.app`). Note: no `vercel.json` in repo — deployed via Vercel UI. **The 2026-04-16 design refresh has not yet been deployed.**

---

## Load Test Results (2026-04-12)

Full report: `docs/load-tests/2026-04-12-results.md`

| Phase | Target | Result |
|-------|--------|--------|
| Phase 1: Baseline | 10-200 RPS | PASS (0% errors, p99 <5s) |
| Phase 2: Break-point | 250-450 RPS | Ceiling ~400 RPS (rate limiter bottleneck) |
| Phase 5: SLO validation | 50 RPS x 5 min | PASS (0% errors, p99 297ms) |
| Phase 6: Exact payment | Real USDC | PASS (5/5 successful) |
| Phase 7: Per-provider | Real USDC x 5 providers | PASS (24/25 successful) |

**Provider status (all 5 verified with real payments):** xAI, Google, Anthropic, OpenAI, DeepSeek

**Bugs fixed during testing:**
- Provider model prefix stripping (OpenAI, xAI, DeepSeek sent `provider/model` to upstream)
- Google response parsing (`GeminiPart.text` required but newer models return `thought` parts)
- Anthropic model name (`claude-3-5-haiku-20241022` → `claude-haiku-4-5-20251001`)

---

## Test Counts (run `cargo test` to verify — these go stale)

Last verified 2026-04-08:

```
gateway unit:        401
gateway integration: 122
router:               13
protocol:             18
x402:                 99
cli:                  30  (fully tested, 8 commands)
───────────────────────
workspace total:     683

escrow (standalone):  21
dashboard (vitest):   82
python sdk:           63
go sdk:               58  (53 pass, 5 skip/live-gated)
```

---

## Deployment

| Resource | Location | Status |
|----------|----------|--------|
| **Gateway** | `rustyclawrouter-gateway.fly.dev` | Running (ord region, shared-cpu-1x/512MB) |
| **PostgreSQL** | `solvela-db` on Fly.io | Running (Postgres 17.2) |
| **Redis** | Upstash (`solvela-cache`) | Running (ord + iad) |
| **Dashboard** | `solvela.vercel.app` | Deployed |
| **Terminal backend** | `rclawterm-gateway.fly.dev` | Running (ord, 2 machines) |

### Secrets on Fly.io

All 5 provider keys set and verified working (OpenAI, Anthropic, Google, xAI, DeepSeek) — refreshed 2026-04-12. Solana config set (RPC, recipient wallet, USDC mint, escrow program, fee payer key). Database + Redis URLs set. Admin token rotated 2026-03-31. Note: Fly app is still `rustyclawrouter-gateway`, not yet renamed to `solvela-gateway`.

---

## What's NOT Done

### Immediate

- **MCP server signing**: Stub signing intentional (agent-only protocol).
- **Deploy 2026-04-16 design refresh**: dashboard restyle is committed locally but not yet deployed to `solvela.vercel.app`.
- **Capture dashboard screenshots** for embedding in `rcr-docs-site` Enterprise pages — dashboard is themed and seeded with convincing fake data, ready to shoot.

### Sister repo — `rcr-docs-site` (the real docs site)

`~/projects/rcr-docs-site/` is the canonical Solvela docs site (target domain `docs.solvela.ai`). Now git-initialized with three commits and a complete redesign as of 2026-04-16:

- **Editorial-serif typography** (Source Serif 4 at light weight 300 for h1, 500 for headings)
- **Salmon eyebrow** (`#FE8181`) on every section title; salmon underline on active nav tab
- **Terminal-window card pattern** with title bar + screen-area radial dot pattern
- **Three top tabs**: Documentation / API Reference / **Enterprise** (new — 7 pages of org/team/api-key/audit/budget/analytics docs pulled from `crates/gateway/src/routes/orgs/`)
- **`UpgradeCta` component** on every Enterprise page links to `solvela.ai/pricing` placeholder

The dashboard now shares the same design tokens (Source Serif 4, salmon accent, terminal-card classes) so docs.solvela.ai and app.solvela.ai feel like one product. See `~/projects/rcr-docs-site/HANDOFF.md` for full details.

### Deferred

- **Multi-chain support**: `PaymentVerifier` trait is chain-agnostic by design. Base/EVM implementation deferred.
- **x402 V2 sessions**: V2 adds sessions and service discovery. Wire format migrated but session features not implemented.
- **Load testing**: COMPLETED 2026-04-12. All 7 phases passed. See `docs/load-tests/2026-04-12-results.md`. T1 ceiling ~400 RPS, SLO validated at 50 RPS x 5 min, all 5 providers verified with real USDC payments. CLI features added: `--model` flag, live progress output, `SOLVELA_RATE_LIMIT_MAX` env override.
- **Fly app rename**: `rustyclawrouter-gateway` → `solvela-gateway` (deferred — requires DNS migration)
- **Docs theme rename**: `@rustyclaw/docs-theme` → `@solvela/docs-theme` (rcr-docs-site design system ported into dashboard 2026-04-14)
- **Rate limiter redesign**: Current `tokio::sync::Mutex<HashMap>` is the bottleneck at 400+ RPS. Replace with sharded or Redis-based approach when traffic demands it.
- **Per-user fairness queuing**: Not started.
- **Secret rotation plan**: No automated rotation.
- **API reference docs**: 17 MDX pages written (2026-04-13). Content based on actual codebase. OpenAPI auto-generation via fumadocs-openapi deferred. Welcome page design polish in progress — iterating toward Anthropic-style visual hierarchy.
- **Rust 2021 → 2024 edition**: Planned but not blocking (currently 2021).
- **SDK publishing**: SDKs exist (Python 63 tests, TypeScript, Go, MCP). PyPI/npm/crates.io publishing status unclear.

### Ecosystem (in priority order)

1. Deploy Terminal frontend to Vercel
2. Harden Solvela under real Terminal load
3. Build OpenClaw plugin (`@solvela/router`)
4. ~~Migrate Telsi from BlockRun to Solvela~~ (completed 2026-04-07)
5. Build Sky64 network agent
6. Open-source (`solvela-router`, `solvela-protocol`)

---

## Regulatory Notes

- **Safe (no licensing)**: AP2 discovery endpoints, x402 crypto settlement (wallet-to-wallet), mandate verification as metadata
- **DO NOT build (triggers MSB + 49 state licenses)**: Card payment processing, fiat ↔ crypto conversion, custodial fund holding
- **Gray area**: Anchor escrow PDAs (trustless, PDA-controlled) — FinCEN guidance on custodial wallets is evolving. Escrow deployed to mainnet 2026-04-08 with upgrade authority retained.
- **Watch**: California DFAL takes effect July 2026.

---

## Key Files

| File | Purpose |
|------|---------|
| `CLAUDE.md` | How to work in the repo (conventions, architecture, commands) |
| `HANDOFF.md` | This file — current project state |
| `CHANGELOG.md` | What changed and when |
| `.claude/plan/rustyclawrouter.md` | Master implementation plan |
| `config/models.toml` | Model registry + pricing |
| `.env.example` | All env vars documented |
