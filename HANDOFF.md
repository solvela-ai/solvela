# HANDOFF.md — Solvela Current State

> **Single source of truth** for project status. See `CLAUDE.md` for how to work in the repo. See `CHANGELOG.md` for history.
> **Last verified:** 2026-04-12 (from actual repo inspection, not docs)

---

## What Is This

Solvela (formerly RustyClawRouter) is a Solana-native LLM payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via x402. Revenue: 5% fee per call.

Part of the **solvela.ai** ecosystem:

| Product | Purpose | Status |
|---------|---------|--------|
| **Solvela** (solvela.ai) | x402 payment gateway, SDKs, CLI, escrow, dashboard (this repo) | Live on Fly.io + Vercel |
| **RustyClaw Terminal** (rustyclaw.ai) | Crypto trading terminal + AI agent | Live on Vercel + Fly.io |
| **Telsi.ai** | Multi-tenant AI assistant SaaS | Live on BlockRun, planned Solvela migration |

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

Trustless USDC-SPL escrow: deposit/claim/refund. PDA vault with timeout refunds. Not a workspace member (dep conflicts).

**Escrow deployed to Solana mainnet** (Apr 8). Program ID: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`.

### SDKs

Python (`sdks/python/`), TypeScript (`sdks/typescript/`), Go (`sdks/go/`), MCP server (`sdks/mcp/`). Packages renamed to `solvela-sdk` / `@solvela/sdk` / `@solvela/mcp-server`.

### Dashboard

Next.js 16 + Tailwind + Recharts. 5 pages: Overview, Usage, Models, Wallet, Settings. Deployed to Vercel (`solvela.vercel.app`). Note: no `vercel.json` in repo — deployed via Vercel UI.

### Documentation Site

Separate repo (`rcr-docs-site`). Next.js 16 + Fumadocs MDX. 18 pages across 5 sections (Getting Started, Core Concepts, API Reference, SDK Guides, Operations). Deployed to Vercel as `solvela-docs` at `docs.solvela.ai`. Uses `@rustyclaw/docs-theme` shared theme library (separate repo: `docs-theme`). Also includes an in-repo mdBook at `docs/book/` for offline/developer reference.

### CLI Load Test Framework

Built-in load testing via `solvela loadtest`. Constant-arrival-rate dispatcher, latency histograms, backpressure tracking, Prometheus scraper, SLO validation. Payment strategies: dev-bypass, exact (SPL TransferChecked), escrow (Anchor deposit). Terminal + JSON report output.

### CLI Escrow Recovery

`solvela recover` subcommand refunds expired escrow PDAs. Shows atomic + decimal USDC amounts. `--scheme` flag on `solvela chat` selects exact vs escrow payment.

---

## Test Counts (run `cargo test` to verify — these go stale)

Last verified 2026-04-12:

```
gateway unit:        401
gateway integration: 122
router:               13
protocol:             18
x402:                 99
cli:                  30
───────────────────────
workspace total:     683

escrow (standalone):  21
dashboard (vitest):   82
```

---

## Deployment

| Resource | Location | Status |
|----------|----------|--------|
| **Gateway** | `solvela-gateway.fly.dev` | Running (ord region) |
| **PostgreSQL** | `solvela-db` on Fly.io | Running (Postgres 17.2) |
| **Redis** | Upstash (`solvela-cache`) | Running (ord + iad) |
| **Dashboard** | `solvela.vercel.app` | Deployed |
| **Docs site** | `docs.solvela.ai` | Deployed on Vercel |
| **Terminal backend** | `rclawterm-gateway.fly.dev` | Running (ord, 2 machines) |
| **Terminal frontend** | `rustyclaw.ai` | Deployed on Vercel |
| **Escrow program** | Solana mainnet | `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` |

### Secrets on Fly.io

All 5 provider keys set (OpenAI, Anthropic, Google, xAI, DeepSeek). Solana config set (RPC, recipient wallet, USDC mint, escrow program, fee payer key). Database + Redis URLs set. Admin token rotated 2026-03-31. Env vars use `SOLVELA_` prefix (legacy `RCR_` accepted with deprecation warnings).

---

## What's NOT Done

### Immediate

- **SDK repo renames**: External SDK repos (`rustyclaw-go`, `rustyclaw-python`, `rustyclaw-ts`, `RustyClawClient`) still use RustyClaw naming. Need rename to `solvela-*`.
- **Python SDK directory**: In-tree `sdks/python/` package directory still named `rustyclawrouter/`, should be `solvela/`.
- **Docs theme rename**: `@rustyclaw/docs-theme` package needs rename to `@solvela/docs-theme`.
- **Docs repos not git-tracked**: `rcr-docs-site` and `docs-theme` have no git repos initialized.

### Deferred

- **Multi-chain support**: `PaymentVerifier` trait is chain-agnostic. Base/EVM deferred.
- **x402 V2 sessions**: Wire format migrated, session features not implemented.
- **Per-user fairness queuing**: Not started.
- **Secret rotation plan**: No automated rotation.
- **API reference docs**: Incomplete.
- **Rust 2021 → 2024 edition**: Planned but not blocking.
- **SDK publishing**: `@rustyclaw/sdk` on npm. PyPI and crates.io not published.
- **Mainnet load testing**: CLI load test framework built but not run against production yet.

### Ecosystem (in priority order)

1. Harden Solvela under real Terminal load
2. Build OpenClaw plugin (`@solvela/clawrouter`)
3. Migrate Telsi from BlockRun to Solvela
4. Build Sky64 network agent
5. Open-source (`solvela-router`, `solvela-protocol`)

---

## Regulatory Notes

- **Safe (no licensing)**: AP2 discovery endpoints, x402 crypto settlement (wallet-to-wallet), mandate verification as metadata
- **DO NOT build (triggers MSB + 49 state licenses)**: Card payment processing, fiat ↔ crypto conversion, custodial fund holding
- **Gray area**: Anchor escrow PDAs (trustless, PDA-controlled) — FinCEN guidance on custodial wallets is evolving. Attorney consultation pending (was scheduled 2026-04-06).
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
