# HANDOFF.md — Solvela Current State

> **Single source of truth** for project status. See `CLAUDE.md` for how to work in the repo. See `CHANGELOG.md` for history.
> **Last verified:** 2026-04-18 (migration-runner fix merged to main & live in prod via PR #17; three-subdomain architecture re-verified end-to-end — `api.solvela.ai` → Fly gateway HTTP 200, `app.solvela.ai` → Vercel dashboard 307→overview, `docs.solvela.ai` → Vercel docs HTTP 200, `solvela.ai` apex → 307→/docs. Prior verification on 2026-04-17 covered Fly gateway+DB cutover: `rustyclawrouter-gateway` → `solvela-gateway`, `rustyclawrouter-db` → `solvela-db`, `sqlx::migrate!` wired so all 7 migration files apply — `solvela-db` has 10 tables + `_sqlx_migrations` tracker with all 7 versions recorded.)

---

## What Is This

Solvela is a Solana-native LLM payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via x402. Revenue: 5% fee per call.

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
| **Enterprise (orgs)** | 12 endpoints under `/v1/orgs/...` | Routes mounted, API key auth gate firing (401s as expected); tables now exist after 2026-04-17 migration-runner fix — full CRUD not yet exercised with real org traffic |
| **Wallet/Stats** | `GET /v1/wallet/{addr}/stats`, `GET /v1/admin/stats` | Working |
| **Infrastructure** | `GET /health`, `GET /pricing`, `GET /metrics`, `GET /v1/nonce`, `GET /v1/supported` | Working |

### Middleware Stack

Rate limiting, API key extraction (org-scoped), x402 payment extraction, CORS, security headers (CSP, X-Frame-Options, HSTS), request ID tracking, concurrency limiting, global timeout, panic handler.

### Database (7 migrations)

Core tables (wallet_budgets, escrow, claims), escrow claim queue, session tracking, retry scheduling, organizations/teams/members/API keys, audit logs, hourly spend limits.

### Escrow Program (Anchor, standalone)

Trustless USDC-SPL escrow: deposit/claim/refund. PDA vault with timeout refunds. Not a workspace member (dep conflicts). **Deployed to mainnet** (`9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`) with upgrade authority retained by deployer (`B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`).

### SDKs

Python (`sdks/python/`), TypeScript (`sdks/typescript/`), Go (`sdks/go/`), MCP server (`sdks/mcp/`). Repos renamed to `solvela-ts`, `solvela-python`, `solvela-go`, `solvela-client` with 301 redirects from old `rustyclaw-*` names live on GitHub.

### Dashboard & Documentation Site

Next.js 16 + Tailwind v4 + Fumadocs (core + mdx) + Recharts. Single codebase serving three subdomains via `dashboard/src/proxy.ts` (Next.js 16 middleware pattern):
- `solvela.ai/` → 307 redirect to `/docs`
- `docs.solvela.ai/*` → rewrite to `/docs/*`
- `app.solvela.ai/*` → rewrite to `/dashboard/*`

**Design system (2026-04-16 audit pass):** Source Serif 4 (serif display font), salmon accent `#FE8181` for eyebrow labels. Terminal-window card pattern (`.terminal-card`) with titlebar + radial dot screen. Nine focused design audit passes completed:
- **/harden** — focus-visible rings, ≥40px touch targets, contrast bumps, ARIA live regions, role attributes, `type="button"` semantics
- **/clarify** — Topbar title dedup, sentence-case wallet titles
- **/optimize** — `router.refresh()` instead of `window.location.reload()`
- **/distill** — wallet escrow 4-tile → semantic `<dl>/<dt>/<dd>`
- **/extract** — new `<TerminalCard>` component (13 inline copies replaced)
- **/typeset** — `--text-xxs` (11px) token, `.metric-xl/lg/md` serif classes
- **/arrange** — Overview stat grid redesigned (hero treasury + 2×2 support)
- **/normalize** — stale color tokens deleted after confirming unused
- **/polish** — 6 surgical consistency fixes, Settings grouped into 4 labeled sections

**Docs engine:** Fumadocs-core + fumadocs-mdx. Custom UI components ported from `rcr-docs-site` (NOT using fumadocs-ui). Fonts: DM Sans (body), Archivo (legacy display), JetBrains Mono (data/labels), Source Serif 4 (headings). Shiki syntax themes (solvela-dark/light).

**Docs pages:** 8 main pages (Welcome, Quickstart, Architecture, Request Flow, Payment Flow, Pricing) + 5 MDX components (`UpgradeCta`, `FlowSteps`, `HeroSplit`, `TierCards`, `LinkMap`) + 7 Enterprise pages (Organizations, Teams, API Keys, Audit, Budgets, Analytics) ported from `rcr-docs-site` — 39 total static routes.

**Dashboard pages (5):** Overview, Usage, Models, Wallet, Settings — all themed with terminal-card pattern. Seeded with convincing mock data (~$247.83 spend, 12.4k requests, realistic curves) — ready for marketing screenshots.

**Subdomain architecture:** Implemented via `proxy.ts` (Next.js 16 middleware). Host allowlist and iframe `allow` tightening deferred as nits. "Already-prefixed" guard prevents double-rewrite (`docs.solvela.ai/docs/quickstart`).

Deployed to Vercel (`solvela.vercel.app`). **The 2026-04-16 design refresh and subdomain middleware are deployed and live.**

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

Last verified 2026-04-18 on merged `main`:

```
gateway unit:        404  (was 405 before migration-runner fix removed test_migration_sql_is_valid)
gateway integration: 122
migrations test:       1  (#[ignore]-gated, opt-in via TEST_DATABASE_URL + -- --ignored)
router:               13
protocol:             18
x402:                 99
cli:                  30  (fully tested, 8 commands)

escrow (standalone):  21
dashboard (vitest):   82
python sdk:           63
go sdk:               58  (53 pass, 5 skip/live-gated)
```

---

## Deployment

| Resource | Location | Status |
|----------|----------|--------|
| **Gateway** | `solvela-gateway.fly.dev` (serves `api.solvela.ai`) | Running (ord region, shared-cpu-1x/512MB, 2 machines HA) |
| **Dashboard + Docs** | `solvela.vercel.app` (+ `solvela.ai`, `docs.solvela.ai`, `app.solvela.ai` subdomains) | Deployed, three-subdomain routing live |
| **PostgreSQL** | `solvela-db` on Fly.io | Running (Postgres-flex 17.2, fresh cluster, `solvela_gateway` user owns `solvela_gateway` DB) |
| **Redis** | Upstash (`solvela-cache`) | Running (ord + iad) |
| **Terminal backend** | `rclawterm-gateway.fly.dev` | Running (ord, 2 machines) |

### Secrets on Fly.io

All 5 provider keys set on `solvela-gateway` and verified working (OpenAI, Anthropic, Google, xAI, DeepSeek) — refreshed 2026-04-12. Solana config set using `SOLVELA_*` prefix (RPC, recipient wallet, USDC mint, escrow program, fee payer key). DATABASE_URL points at `solvela-db.flycast/solvela_gateway`. Redis URL set. Admin token rotated 2026-03-31.

**Canonical API URL:** `api.solvela.ai` — live on `solvela-gateway` via Cloudflare DNS (A `66.241.125.165`, AAAA `2a09:8280:1::104:9270:0`), Let's Encrypt cert.

---

## What's NOT Done

### Immediate / Cleanup

- **rcr-docs-site archive:** Sister repo `~/projects/rcr-docs-site/` is now a strict subset of this repo. Safe to archive or delete.
- **Local directory rename:** `~/projects/RustyClawRouter/` → `~/projects/solvela/` (optional; git remote already updated).
- **Dashboard 2026-04-16 screenshots:** Capture for embedding in docs (dashboard is themed and ready).

### Post-migration cleanup (remaining after 2026-04-17 cutover)

- **Old gateway app:** `rustyclawrouter-gateway` still exists as rollback safety net. Destroy once confident: `flyctl apps destroy rustyclawrouter-gateway --yes`.
- **Old DB cluster:** `rustyclawrouter-db` still exists with its original data (2 tables, 0 rows — `rustyclawrouter_gateway` user+DB intact, `solvela_gateway` user dropped). Destroy once confident: `flyctl apps destroy rustyclawrouter-db --yes`.
- **ACME CNAME:** `_acme-challenge.api.solvela.ai` in Cloudflare was used to pre-issue the cert via DNS-01. Safe to delete once HTTP-01 has handled at least one renewal.
- **Migration runner:** Fixed 2026-04-17 (see `docs/superpowers/plans/2026-04-17-fix-migration-runner.md`). `run_migrations()` now uses `sqlx::migrate!` which embeds all migration files at compile time and tracks applied versions in `_sqlx_migrations`. Dockerfile also now copies `migrations/` into the build context. Canonical table list lives in `crates/gateway/tests/migrations.rs::EXPECTED_TABLES`; run `\dt` on the DB to verify live state. Enterprise endpoints now return proper auth errors (401) instead of `relation does not exist` 500s.

### Deferred

- **Multi-chain support:** `PaymentVerifier` trait is chain-agnostic by design. Base/EVM implementation deferred.
- **x402 V2 sessions:** V2 adds sessions and service discovery. Wire format migrated but session features not implemented.
- **Rate limiter redesign:** Current `tokio::sync::Mutex<HashMap>` is the bottleneck at 400+ RPS. Replace with sharded or Redis-based approach when traffic demands it.
- **Per-user fairness queuing:** Not started.
- **Secret rotation plan:** No automated rotation.
- **Rust 2021 → 2024 edition:** Planned but not blocking (currently 2021).
- **SDK publishing:** SDKs exist (Python 63 tests, TypeScript, Go, MCP). New repo names are `solvela-ts`, `solvela-python`, `solvela-go`, `solvela-client`; PyPI/npm/crates.io publishing status TBD.
- **Proxy security nits:** Host allowlist and iframe `allow` tightening (defer after launch).
- **Private-key rotation warnings:** SDK docs deferred (wallet security caveats for client-side signing).

### Ecosystem (in priority order)

1. Deploy Terminal frontend to Vercel
2. Harden Solvela under real Terminal load
3. Build OpenClaw plugin (`@solvela/router`)
4. ~~Migrate Telsi from BlockRun to Solvela~~ (completed 2026-04-07)
5. Build Sky64 network agent
6. Open-source (`solvela-router`, `solvela-protocol`)

---

## Regulatory Notes

- **Safe (no licensing):** AP2 discovery endpoints, x402 crypto settlement (wallet-to-wallet), mandate verification as metadata
- **DO NOT build (triggers MSB + 49 state licenses):** Card payment processing, fiat ↔ crypto conversion, custodial fund holding
- **Gray area:** Anchor escrow PDAs (trustless, PDA-controlled) — FinCEN guidance on custodial wallets is evolving. Escrow deployed to mainnet 2026-04-08 with upgrade authority retained.
- **Watch:** California DFAL takes effect July 2026.

---

## Key Files

| File | Purpose |
|------|---------|
| `CLAUDE.md` | How to work in the repo (conventions, architecture, commands) |
| `HANDOFF.md` | This file — current project state |
| `CHANGELOG.md` | What changed and when |
| `.claude/plan/solvela.md` | Master implementation plan |
| `config/models.toml` | Model registry + pricing |
| `.env.example` | All env vars documented |
| `dashboard/src/proxy.ts` | Next.js 16 middleware for three-subdomain routing |
