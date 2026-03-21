# HANDOFF.md — RustyClawRouter Continuation Guide

> **Start here.** This document captures full context so a fresh agent can continue without ramp-up time.

---

## Goal

RustyClawRouter (RCR) is a Solana-native LLM payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via the x402 protocol. No API keys, no accounts, just wallets. Revenue: 5% fee on every proxied LLM call.

RCR is one of three products under **rustyclaw.ai**:

| Product | Purpose | Revenue | Status |
|---------|---------|---------|--------|
| **RustyClaw Terminal** | Crypto trading terminal + AI agent | $49-179/mo (Whop) | Built, not deployed |
| **RustyClawRouter** | LLM payment gateway (this repo) | 5% fee per LLM call | Deployed on Fly.io |
| **Telsi.ai** | Multi-tenant AI assistant SaaS | $59-229/mo (Stripe) | Live on BlockRun, planned RCR migration |

**Ecosystem strategy:** See `/home/kennethdixon/projects/rustyclaw/docs/superpowers/specs/2026-03-17-rustyclaw-ecosystem-design.md` for full context on build-vs-adopt, GitHub strategy, Telsi migration path, and competitive landscape.

---

## Current Progress

### What's Complete (Phases 1-3)

| Phase | Description | Tests |
|-------|-------------|-------|
| 1 | **Core Gateway + x402 Payments** — Axum HTTP server, x402 middleware, Solana payment verification, 5 provider adapters (OpenAI, Anthropic, Google, xAI, DeepSeek), rate limiting, usage tracking | 267 gateway + 110 x402 |
| 2 | **Smart Router + Caching** — 15-dimension scorer, 4 profiles (ECO/AUTO/PREMIUM/FREE), 10 aliases, Redis response cache, provider fallback + circuit breaker | 13 router + 18 protocol |
| 3 | **SDKs + CLI** — Python (63 tests), TypeScript (19 tests), Go (12 tests), Rust CLI (wallet, models, chat, health, stats, doctor) | 99 cli + 94 SDK tests |

**Total: 507 Rust tests + 94 SDK tests, all passing.**

### Deployment Status (as of 2026-03-18)

| Resource | Status | Details |
|----------|--------|---------|
| **RCR Gateway** | Running | `rustyclawrouter-gateway.fly.dev`, 1 machine (ord), health returns 200 |
| **PostgreSQL** | Running | `rustyclawrouter-db` on Fly.io, Postgres 17.2, 3/3 health checks passing |
| **Upstash Redis** | Running | `rustyclawrouter-cache`, pay-as-you-go, ord + iad regions |

### Deployment Blockers

| # | Blocker | Severity | Fix |
|---|---------|----------|-----|
| 1 | **Fee-payer wallet has 0 SOL** — can't settle x402 payments on-chain | LOW | Soft enforcement mode — requests proceed. Fund ~0.1 SOL when ready for real settlement |
| 2 | ~~No LLM provider API keys~~ | ~~CRITICAL~~ | **RESOLVED 2026-03-18** — OpenAI + Google keys set |
| 3 | ~~Placeholder services failing~~ | ~~WARN~~ | **RESOLVED 2026-03-18** — services.toml commented out, redeployed |

### Secrets Currently Set on Fly.io

```
DATABASE_URL                  ✅
REDIS_URL                     ✅
RCR_SOLANA__RECIPIENT_WALLET  ✅
RCR_SOLANA__RPC_URL           ✅ (mainnet via Helius)
RCR_SOLANA__USDC_MINT         ✅
RCR_SOLANA__ESCROW_PROGRAM_ID ✅
RCR_SOLANA__FEE_PAYER_KEY     ✅
OPENAI_API_KEY                ✅ (set 2026-03-18)
GOOGLE_API_KEY                ✅ (set 2026-03-18)
RCR_INTERNAL_SERVICE_KEY      ✅ (shared with rclawterm-gateway, set 2026-03-18)
ANTHROPIC_API_KEY             ✗ NOT SET
XAI_API_KEY                   ✗ NOT SET
DEEPSEEK_API_KEY              ✗ NOT SET
```

---

## What's NOT Done Yet

### Phase 4: Anchor Escrow Program — NOT STARTED
- Trustless on-chain payment escrow (PDA vault, deposit/claim/refund)
- Skeleton exists at `programs/escrow/` but no implementation
- **Dependency:** Not needed for initial launch — V1 direct settlement works

### Phase 5: Dashboard + Enterprise — NOT STARTED
- Admin dashboard, analytics, team billing, SSO
- **Dependency:** Not needed until RCR has external customers

### Phase 6: x402 Service Marketplace — NOT STARTED
- Service registry/discovery (`GET /v1/services`), proxy mode
- Config files exist (`config/services.toml`) but endpoints not implemented

### Other Deferred Items
- **x402 V2 Migration** — Currently V1 headers. V2 + session/JWT needed when agent-to-RCR integration goes live. High complexity.
- **Multi-chain support** — Base/EVM compatibility deferred. `PaymentVerifier` trait is chain-agnostic by design.
- Load testing, per-user fairness queuing, secret rotation plan
- Domain-specific model evaluation
- Complete API reference documentation, deployment runbook

---

## Coordination with Other Projects

### RustyClaw Terminal (`/home/kennethdixon/projects/rustyclaw`)
- Terminal's AI agent routes through RCR via `RcrProvider` in `rclawterm-agent`
- **Terminal backend deployed** (2026-03-18): `rclawterm-gateway.fly.dev`, 2 machines (ord), health OK
- Terminal uses the same Upstash Redis instance (`rustyclawrouter-cache`)
- Exchange execution disabled until Next.js app is on Vercel (`INTERNAL_API_URL` + `INTERNAL_API_KEY` not set)
- **Next:** Deploy Next.js frontend to Vercel, set `NEXT_PUBLIC_GATEWAY_WS_URL=wss://rclawterm-gateway.fly.dev`

### Telsi.ai (`/home/kennethdixon/projects/clawstack`)
- Currently routes through BlockRun (x402 USDC on Base chain)
- Planned migration to RCR: Shadow → Canary → Flip → Clean
- **Prerequisites before migration:**
  - RCR handles real traffic (tested with Terminal first)
  - RCR supports Telsi's models (Gemini Flash, DeepSeek, Claude Sonnet, Opus)
  - `openclaw-plugin-rcr` built and installable on VPS
  - Telsi wallet switched from Base USDC to Solana USDC
- **Not started yet.** Terminal comes first.

### Ecosystem Priority Order (from design spec)

1. Get Terminal live (deploy, first subscribers)
2. Harden RCR under real Terminal load
3. Build OpenClaw plugin (`@rustyclaw/clawrouter`)
4. Migrate Telsi (Shadow → Canary → Flip → Clean)
5. Build Sky64 network agent (third vertical)
6. Go public (open-source `rcr-router`, `rcr-protocol`)

---

## Project Structure

```
crates/
  gateway/         Axum HTTP server — routes, middleware, provider adapters, cache, usage tracking
  x402/            x402 protocol — Solana payment verification, facilitator, escrow types
  router/          Smart routing — 15-dimension scorer, profiles, model registry
  protocol/        Shared types — ChatRequest, ChatResponse, CostBreakdown (rustyclaw-protocol on crates.io)
  cli/             CLI tool (rcr) — wallet, models, chat, health, stats, doctor
programs/
  escrow/          Anchor escrow program (Phase 4 — not started)
sdks/
  python/          pip install rustyclawrouter (63 tests)
  typescript/      npm install @rustyclawrouter/sdk (19 tests)
  go/              go get github.com/rustyclawrouter/sdk-go (12 tests)
config/
  models.toml      Model registry + pricing (26 models, 5 providers, 5% platform fee)
  default.toml     Gateway configuration
  services.toml    x402 service marketplace registry (Phase 6)
```

---

## Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | Rust + Axum | Sub-microsecond routing. Memory safety. Solana ecosystem native. |
| D2 | 15-dimension scorer | Token count, code detection, reasoning markers, technical terms, etc. <1µs per request. |
| D3 | x402 V1 direct settlement | Pre-signed SPL TransferChecked. Facilitator settles. V2 deferred. |
| D4 | 5% platform fee | Transparent, shown in all responses. Sustainable revenue. |
| D5 | Chain-agnostic PaymentVerifier trait | Solana-first, but trait designed for future EVM support. |
| D6 | Provider adapters (not passthrough) | Each provider has format translation. OpenAI-compatible external API. |
| D7 | Redis for cache + rate limiting | Hot-path data. 10min TTL default. Per-wallet + per-IP rate limits. |
| D8 | PostgreSQL for usage tracking | Async writes only. Never on critical path. Per-wallet/model/provider. |
| D9 | Independent facilitator | No BlockRun or Coinbase CDP dependency. Own payment verification. |

---

## Fly.io Infrastructure

### Apps

| App | Purpose | Region | Status |
|-----|---------|--------|--------|
| `rustyclawrouter-gateway` | RCR HTTP gateway | ord (Chicago) | 1 machine running |
| `rustyclawrouter-db` | PostgreSQL 17.2 | ord | Primary running, 3/3 checks |
| `rustyclawrouter-cache` | Upstash Redis | ord + iad | Pay-as-you-go |

### Deploy Commands

```bash
# Deploy gateway (from this repo root)
cd crates/gateway && fly deploy -a rustyclawrouter-gateway

# Set secrets
fly secrets set -a rustyclawrouter-gateway \
  OPENAI_API_KEY=sk-... \
  ANTHROPIC_API_KEY=sk-ant-... \
  GOOGLE_API_KEY=... \
  XAI_API_KEY=... \
  DEEPSEEK_API_KEY=...

# Check status
fly status -a rustyclawrouter-gateway
fly logs -a rustyclawrouter-gateway --no-tail

# Database
fly status -a rustyclawrouter-db
fly postgres connect -a rustyclawrouter-db

# Redis
fly redis list
```

---

## Test Commands

```bash
# Rust (507 tests across 5 crates)
cargo test                    # All crates
cargo test -p gateway         # Gateway (267 tests)
cargo test -p x402            # x402 protocol (110 tests)
cargo test -p router          # Smart router (13 tests)
cargo test -p rcr-cli         # CLI (99 tests)
cargo test -p rustyclaw-protocol  # Protocol (18 tests)

# Lint
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings

# SDKs
cd sdks/python && python -m pytest       # 63 tests
cd sdks/typescript && npm test           # 19 tests
cd sdks/go && go test ./...              # 12 tests

# Health check (live)
curl https://rustyclawrouter-gateway.fly.dev/health
```

---

## Key Files

| File | Purpose |
|------|---------|
| `.claude/plan/rustyclawrouter.md` | Master implementation plan (885 lines) |
| `AGENTS.md` | AI agent coding guidelines |
| `HANDOFF.md` | This file — continuation guide |
| `config/models.toml` | Model registry (pricing, capabilities, routing profiles) |
| `config/default.toml` | Gateway configuration |
| `config/services.toml` | x402 service marketplace config |
| `.env.example` | Environment variable template |
| `docker-compose.yml` | Local dev (Redis + PostgreSQL) |

---

## What's Next (User Priority)

**User decision (2026-03-18):** Backend + RCR functionality first. Payments (Stripe/Whop) deferred.

1. **Unblock RCR** — Add LLM provider API keys, fund fee-payer wallet, fix placeholder services
2. **Test end-to-end** — SDK → RCR gateway → real LLM provider → response with cost breakdown
3. **Deploy Terminal backend** — `rclawterm-gateway` on Fly.io (separate from RCR gateway)
4. **Wire Terminal → RCR** — Verify the full flow: Terminal agent → RCR → LLM → response
5. **Then:** Landing page deploy, Stripe products, Whop listing
