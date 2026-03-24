# HANDOFF.md — RustyClawRouter Continuation Guide

> **Start here.** This document captures full context so a fresh agent can continue without ramp-up time.
> **Last updated:** 2026-03-24

---

## Goal

RustyClawRouter (RCR) is a Solana-native LLM payment gateway. AI agents pay for LLM API calls with USDC-SPL on Solana via the x402 protocol. No API keys, no accounts, just wallets. Revenue: 5% fee on every proxied LLM call.

RCR is one of three products under **rustyclaw.ai**:

| Product | Purpose | Revenue | Status |
|---------|---------|---------|--------|
| **RustyClaw Terminal** | Crypto trading terminal + AI agent | $49-179/mo (Whop) | Built, not deployed |
| **RustyClawRouter** | LLM payment gateway (this repo) | 5% fee per LLM call | Deployed on Fly.io |
| **Telsi.ai** | Multi-tenant AI assistant SaaS | $59-229/mo (Stripe) | Live on BlockRun, planned RCR migration |

**Ecosystem strategy:** See `/home/kennethdixon/projects/rustyclaw/docs/superpowers/specs/2026-03-17-rustyclaw-ecosystem-design.md` for full context.

---

## Current Progress

### What's Complete

| Phase | Description | Key Components |
|-------|-------------|----------------|
| 1-3 | **Core Gateway + x402 + Smart Router + SDKs** | Axum server, x402 middleware, 5 providers, 15-dim scorer, 4 routing profiles, Redis cache, circuit breaker, Python/TS/Go SDKs, CLI |
| 4 | **Anchor Escrow Program** | Deposit/claim/refund instructions, PDA vault, timeout-based refunds, events. `programs/escrow/` |
| 8 | **Escrow Hardening** | Claim queue (PostgreSQL), claim processor (background), fee payer pool rotation (8 wallets), durable nonces, circuit breaker, exponential backoff, escrow metrics |
| 9 | **Service Marketplace** | External service proxy, admin registration API, background health checker |
| 12 | **Prometheus Monitoring** | 15 metrics, `/metrics` endpoint, request/payment/provider/cache/escrow/infra instrumentation |
| 13 | **Documentation** | Comprehensive docs overhaul |
| 14 | **Production Hardening** | CatchPanicLayer, request timeout, connection limits, shared HTTP clients, graceful shutdown |
| G | **Gateway Changes** | Debug headers (`X-RCR-*`), stats endpoint (`GET /v1/stats`), session ID tracking, SSE heartbeat, nonce endpoint |
| — | **Security Audits** | Multiple rounds: 7 CRITICAL, 7 HIGH, 4 HIGH, 12 MEDIUM — all resolved |
| — | **Chat Route Refactor** | Monolithic `chat.rs` (2405 lines) → `chat/` module directory (mod.rs, cost.rs, payment.rs, provider.rs, response.rs) |
| — | **Audit Bug Fixes** | E1 retry unwrap, S1 DNS rebinding TOCTOU, S2 replay TTL, SSE buffer optimization, shared HTTP clients |

**Total: 516 Rust tests + 94 SDK tests, all passing. Lint clean (fmt + clippy).**

### Test Breakdown

```
gateway:   276 tests (unit + integration)
x402:      110 tests
cli:        99 tests
protocol:   18 tests
router:     13 tests
─────────────────
Total:     516 Rust tests
```

### What's NOT Done Yet

#### Phase 5: Dashboard + Enterprise — IN PROGRESS
- Next.js dashboard scaffolded with pages: Overview, Usage, Models, Wallet, Settings
- Charts: spend-chart, requests-bar, model-pie
- Components: shell layout, topbar, sidebar, stat-card, status-dot, badge
- **Still needed:** Connect to real gateway API (currently mock data), enterprise features (team billing, SSO, audit logs)
- **Market research completed:** `docs/research/2026-03-23-ai-agent-payment-infrastructure.md`

#### Other Deferred Items
- **x402 V2 Migration** — V2 launched Dec 2025 (sessions, multi-chain, service discovery). We're on V1.
- **Multi-chain support** — Base/EVM deferred. `PaymentVerifier` trait is chain-agnostic by design.
- **AP2 compatibility** — Google's Agent Payments Protocol has 60+ partners. Consider for Phase 7.
- Load testing, per-user fairness queuing, secret rotation plan
- LiteSVM integration tests for escrow program
- Complete API reference documentation

---

## Competitive Landscape (as of 2026-03-23)

| Competitor | Chain | Escrow | Routing | Users | Funding |
|-----------|-------|--------|---------|-------|---------|
| **BlockRun** | Base only | No | No | Active | Unknown |
| **OpenRouter** | Traditional payments | No | Yes (400+ models) | 5M+ | $40M (a16z, Sequoia) |
| **Stripe** (Agentic Commerce) | Base | No | No | Massive | Public ($91.5B) |
| **Google AP2** | Chain-agnostic | No | No | 60+ enterprise partners | Google |
| **RustyClawRouter** | Solana | **Yes (Anchor)** | **Yes (15-dim)** | 0 | Bootstrapped |

**Our edge:** Trustless Anchor escrow (agents don't overpay), Solana-native (50-70% of x402 volume), Rust performance. See full research in `docs/research/2026-03-23-ai-agent-payment-infrastructure.md`.

---

## Deployment Status (as of 2026-03-18)

| Resource | Status | Details |
|----------|--------|---------|
| **RCR Gateway** | Running | `rustyclawrouter-gateway.fly.dev`, 1 machine (ord), health returns 200 |
| **PostgreSQL** | Running | `rustyclawrouter-db` on Fly.io, Postgres 17.2, 3/3 health checks passing |
| **Upstash Redis** | Running | `rustyclawrouter-cache`, pay-as-you-go, ord + iad regions |

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
RCR_INTERNAL_SERVICE_KEY      ✅ (shared with rclawterm-gateway)
ANTHROPIC_API_KEY             ✗ NOT SET
XAI_API_KEY                   ✗ NOT SET
DEEPSEEK_API_KEY              ✗ NOT SET
```

### Deployment Blockers

| # | Blocker | Severity | Status |
|---|---------|----------|--------|
| 1 | Fee-payer wallet has 0 SOL | LOW | Soft enforcement — requests proceed. Fund ~0.1 SOL when ready for real settlement |

---

## Coordination with Other Projects

### RustyClaw Terminal (`/home/kennethdixon/projects/rustyclaw`)
- Terminal's AI agent routes through RCR via `RcrProvider` in `rclawterm-agent`
- **Terminal backend deployed** (2026-03-18): `rclawterm-gateway.fly.dev`, 2 machines (ord), health OK
- Terminal uses the same Upstash Redis instance (`rustyclawrouter-cache`)
- **Next:** Deploy Next.js frontend to Vercel

### Telsi.ai (`/home/kennethdixon/projects/clawstack`)
- Currently routes through BlockRun (x402 USDC on Base chain)
- Planned migration to RCR: Shadow → Canary → Flip → Clean
- **Not started yet.** Terminal comes first.

### Ecosystem Priority Order

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
  gateway/         Axum HTTP server — routes, middleware, providers, cache, usage, metrics, security
    routes/chat/   Refactored: mod.rs, cost.rs, payment.rs, provider.rs, response.rs
    payment_util.rs  Shared payment extraction (extract_payer_wallet, extract_signer)
  x402/            x402 protocol — Solana verification, escrow (verifier, claimer, claim_queue,
                   claim_processor, PDA), fee payer pool, nonce pool, facilitator
  router/          Smart routing — 15-dimension scorer, profiles, model registry
  protocol/        Shared types — ChatRequest, ChatResponse, CostBreakdown (rustyclaw-protocol)
  cli/             CLI tool (rcr) — wallet, models, chat, health, stats, doctor
programs/
  escrow/          Anchor escrow program — deposit, claim, refund instructions, PDA vault
dashboard/         Next.js 15 + Tailwind + shadcn/ui — Overview, Usage, Models, Wallet, Settings
sdks/
  python/          pip install rustyclawrouter (63 tests)
  typescript/      npm install @rustyclawrouter/sdk (19 tests)
  go/              go get github.com/rustyclawrouter/sdk-go (12 tests)
config/
  models.toml      Model registry + pricing (26 models, 5 providers, 5% platform fee)
  default.toml     Gateway configuration
  services.toml    x402 service marketplace registry
docs/
  plans/           Phase plans (8, 9, 12, 14, G, competitive analysis, SDKs)
  research/        Market research (AI agent payment infrastructure)
```

---

## Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| D1 | Rust + Axum | Sub-microsecond routing. Memory safety. Solana ecosystem native. |
| D2 | 15-dimension scorer | Token count, code detection, reasoning markers, etc. <1µs per request. |
| D3 | x402 V1 direct settlement + escrow | Direct for simple payments, Anchor escrow for trustless settlement. |
| D4 | 5% platform fee | Transparent, shown in all responses. Sustainable revenue. |
| D5 | Chain-agnostic PaymentVerifier trait | Solana-first, but trait designed for future EVM support. |
| D6 | Provider adapters (not passthrough) | Format translation per provider. OpenAI-compatible external API. |
| D7 | Redis for cache + rate limiting | Hot-path data. Per-wallet + per-IP rate limits. |
| D8 | PostgreSQL for usage + claim queue | Async writes only. Never on critical path. Durable escrow claims. |
| D9 | Independent facilitator | Own payment verification. No BlockRun/CDP dependency. |
| D10 | Fee payer pool (up to 8 wallets) | Round-robin rotation, auto-failover, 60s cooldown on failure. |
| D11 | Durable nonces | Eliminates blockhash expiry (60s) for long-lived transactions. |
| D12 | Circuit breaker on claims | 50% failure rate in 5-min window → 60s pause. Prevents cascading failures. |

---

## Fly.io Infrastructure

### Deploy Commands

```bash
# Deploy gateway (from repo root)
cd crates/gateway && fly deploy -a rustyclawrouter-gateway

# Set secrets
fly secrets set -a rustyclawrouter-gateway KEY=value

# Check status
fly status -a rustyclawrouter-gateway
fly logs -a rustyclawrouter-gateway --no-tail

# Database
fly postgres connect -a rustyclawrouter-db

# Health check (live)
curl https://rustyclawrouter-gateway.fly.dev/health
```

---

## Test Commands

```bash
# Rust (516 tests across 5 crates)
cargo test                        # All crates
cargo test -p gateway             # Gateway (276 tests)
cargo test -p x402                # x402 protocol (110 tests)
cargo test -p rustyclawrouter-cli # CLI (99 tests)
cargo test -p rustyclaw-protocol  # Protocol (18 tests)
cargo test -p router              # Smart router (13 tests)

# Escrow (standalone — NOT in workspace)
cargo test --manifest-path programs/escrow/Cargo.toml

# Lint
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings

# Dashboard
npm --prefix dashboard test

# SDKs
cd sdks/python && python -m pytest       # 63 tests
cd sdks/typescript && npm test           # 19 tests
cd sdks/go && go test ./...              # 12 tests
```

---

## Key Files

| File | Purpose |
|------|---------|
| `.claude/plan/rustyclawrouter.md` | Master implementation plan (885 lines) |
| `CLAUDE.md` | AI agent coding guidelines |
| `HANDOFF.md` | This file — continuation guide |
| `config/models.toml` | Model registry (pricing, capabilities, routing profiles) |
| `config/services.toml` | x402 service marketplace config |
| `docs/research/2026-03-23-ai-agent-payment-infrastructure.md` | Market research |
| `docs/plans/` | Phase plans (8, 9, 12, 14, G, competitive analysis) |

---

## What's Next

**Phase 5: Dashboard + Enterprise** — Connect dashboard to real API, polish UI, add enterprise features.

**Strategic priority:** Ship fast, differentiate on escrow ("only gateway where agents don't overpay"), target Solana-native agent builders.

1. Dashboard → real API integration (replace mock data)
2. Deploy dashboard (Vercel)
3. Wire Terminal → RCR for real traffic
4. Consider AP2 compatibility and x402 V2 migration
