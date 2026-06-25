<!-- Generated: 2026-05-27 | Files scanned: 85 | Token estimate: ~900 -->

# Solvela Architecture Codemap

**Last Updated:** 2026-05-27  
**Entry Points:** `crates/gateway/src/main.rs` (HTTP server, Axum)  
**Language:** Rust 2021 + TypeScript/Next.js (dashboard) + Go/Python/TS SDKs

## System Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                     HTTP Clients (Agents)                    │
└────────────┬────────────────────────────────────────────────┘
             │
     ┌───────▼────────┐
     │ Solvela Gateway│ (Axum HTTP server, binary: solvela-gateway)
     │ Port 8402      │
     └────────────────┘
             │
    ┌────────┼──────────────────────────────────────┐
    │        │                                      │
┌───▼──┐  ┌─▼──────┐  ┌──────────┐  ┌────────────┐
│ x402 │  │ Router │  │ Protocol │  │  Providers │
│Proto │  │Scorer  │  │ (types)  │  │(OpenAI,   │
│      │  │        │  │          │  │Anthropic) │
└──────┘  └────────┘  └──────────┘  └────────────┘
    │
┌───▼──────────────────┐
│ Solana x402 Payment  │
│ Escrow + Fee Payer   │
│ Nonce Pool           │
└──────────────────────┘
    │
  USDC-SPL on Solana (Mainnet-Beta)
```

## Workspace Crates

### Binary Crates
- **gateway** — Axum HTTP server; only binary in workspace. Routes, middleware, A2A protocol, orgs/teams/API-keys, chat completions, escrow integration.
- **cli** — `solvela` CLI; separate binary. Wallet, chat, models, health, stats commands.

### Library Crates
- **solvela-x402** — Pure protocol library (no Axum). Solana verification, escrow deposit/claim/refund, fee payer pool, nonce pool, facilitator trait.
- **solvela-router** — Smart request scorer (15 dimensions), routing profiles (eco/auto/premium/free), model registry loader.
- **solvela-protocol** — Shared wire-format types. Chat, payment, settlement, streaming, vision, tools, cost, model metadata. Zero workspace dependencies.

### Standalone Program
- **programs/escrow/** — Anchor program (NOT a workspace member). USDC-SPL escrow with deposit/claim/refund. PDA: `[b"escrow", agent, service_id]`.

## Request Flow: POST /v1/chat/completions

```
Request Parsing
    ↓ (resolve model: aliases, profiles, direct IDs)
Prompt Guard (injection, jailbreak, PII)
    ↓
Has PAYMENT-SIGNATURE?
    ├─ NO  → Return 402 with cost breakdown + accepted schemes
    └─ YES → Decode header (base64 or JSON)
             ↓
         Replay Protection (Redis LRU or in-memory)
             ↓
         Verify via Facilitator (Solana blockhash + signature)
             ↓
         Proxy to LLM Provider (OpenAI, Anthropic, Google, etc.)
             ↓
         Cache Response (Redis)
         Log Spend (PostgreSQL fire-and-forget)
         Fire Escrow Claim (if applicable)
             ↓
         Return JSON or SSE stream
```

## Request Flow: POST /a2a (Agent-to-Agent Protocol)

```
GET /.well-known/agent-card.json   (alias: /.well-known/agent.json)
    ↓ (returns AgentCard with AP2 + x402 extensions)
Agent sends message/send JSON-RPC
    ↓
Gateway computes cost, returns Task (input-required)
    ↓ (cost in x402.payment.required metadata)
Agent signs Solana USDC-SPL transaction
    ↓
Agent sends message/send with taskId + x402.payment.payload
    ↓
Gateway verifies payment, proxies to LLM
    ↓
Return Task (completed) with artifacts + receipt
```

## Module Hierarchy: gateway

### routes/
- **chat/** — Chat completions; handlers in mod.rs, cost.rs, payment.rs, provider.rs, response.rs
- **orgs/** — Org CRUD, teams, API keys, audit logs, budget enforcement, analytics, stats
- **models.rs** — List available models
- **services.rs** — Service marketplace registry (loads `config/services.toml`)
- **escrow.rs** — Escrow config + health
- **escrow_settle.rs** — Escrow claim/settle endpoint
- **health.rs** — Health check (returns 200 OK)
- **metrics.rs** — Prometheus `/metrics` endpoint
- **pricing.rs** — Model pricing endpoint (served at `/pricing`)
- **nonce.rs** — Nonce account status
- **images.rs** — Image proxy (for vision models)
- **admin_stats.rs** — Admin-only system statistics
- **proxy.rs** — Generic per-service x402 proxy
- **debug_headers.rs** — Debug request headers
- **stats.rs** — Wallet-scoped usage stats
- **supported.rs** — List supported models/providers

### middleware/
- **api_key.rs** — `RequireOrg`, `RequireOrgAdmin` extractors; populates `OrgContext`
- **x402.rs** — Payment verification; extracts `PAYMENT-SIGNATURE` header; returns 402 if missing
- **rate_limit.rs** — Per-wallet/per-org rate limiting with RateLimiter + sliding window
- **prompt_guard.rs** — Injection, jailbreak, PII detection
- **metrics.rs** — Prometheus metrics collection (requests, latencies)
- **request_id.rs** — Generates/tracks request IDs via headers

### a2a/
- **handler.rs** — Main A2A request handler (message/send, task state machine)
- **jsonrpc.rs** — JSON-RPC 2.0 parser/dispatcher
- **agent_card.rs** — AgentCard + AP2 + x402 metadata
- **types.rs** — A2A protocol types (Task, Message, Metadata)
- **task_store.rs** — In-memory or DB-backed task storage (session-scoped)

### orgs/
- **models.rs** — Organization, Team, TeamWallet, ApiKey, OrgMember domain models
- **queries.rs** — CRUD operations (find_org, create_team, add_member, etc.)
- **mod.rs** — Re-exports

### providers/
- **openai.rs** — OpenAI v1 API adapter
- **anthropic.rs** — Anthropic API adapter
- **google.rs** — Google Gemini API adapter
- **deepseek.rs** — DeepSeek API adapter
- **xai.rs** — xAI Grok API adapter (if configured)
- **health.rs** — Provider health tracking (latency, error rates)

### Core Modules
- **lib.rs** — `AppState` struct; `build_router()` function
- **main.rs** — Binary entry; config load, middleware stack, server start
- **config.rs** — `AppConfig` (Solana RPC, recipient wallet, USDC mint, server port)
- **error.rs** — `GatewayError` enum with HTTP status mapping
- **payment_util.rs** — Cost calculation helpers
- **secret.rs** — Redacted secret-string wrapper
- **usage.rs** — `UsageTracker` (PostgreSQL spend logs, budget checks)
- **cache/** — `ResponseCache` (Redis + LRU fallback); wallet-agnostic by model+messages+temp; submodules `exact`, `semantic`, `embedder`
- **audit.rs** — Fire-and-forget audit log writer (async, no .await on hot path)
- **security.rs** — Secret redaction, API key verification
- **balance_monitor.rs** — Background task: monitor fee-payer SOL balance (not USDC)
- **services.rs** — `ServiceRegistry` (loads config/services.toml)
- **service_health.rs** — Health tracking for external services
- **session.rs** — Session token generation/verification (HMAC)

## Database Schema (PostgreSQL, optional)

| Table | Purpose | Key Fields |
|-------|---------|-----------|
| `spend_logs` | One row per LLM request | wallet_address, model, provider, input_tokens, output_tokens, cost_usdc, tx_signature, request_id, session_id, created_at |
| `wallet_budgets` | Per-wallet spend limits | wallet_address (PK), hourly/daily/monthly_limit_usdc, total_spent_usdc |
| `organizations` | Billing entity | id (UUID), name, slug (unique), owner_wallet |
| `teams` | Org sub-division | id, org_id (FK), name |
| `org_members` | Wallet→Org mapping | id, org_id (FK), wallet_address, role (owner/admin/member) |
| `team_wallets` | Team→Wallet mapping | id, team_id (FK), wallet_address |
| `team_budgets` | Per-team spend caps | team_id (PK/FK), hourly/daily/monthly_limit_usdc |
| `api_keys` | Org-scoped API credentials | id, org_id (FK), key_hash (unique), key_prefix, role, expires_at, revoked_at |
| `audit_logs` | Action tracking | id, org_id (FK), actor_wallet, actor_api_key (FK), action, resource_type, resource_id, details, ip_address, created_at |
| `escrow_claim_queue` | Pending USDC claims | id, agent_pubkey, service_id (BYTEA), claim_amount (BIGINT atomic), deposited_amount, status, attempts, tx_signature, next_retry_at, updated_at |

Migrations in `migrations/`: 001 (spend_logs, budgets) → 009 (audit actor admin); see [data.md](data.md) for the per-migration breakdown.

## x402 Crate (Protocol)

| Module | Purpose |
|--------|---------|
| `facilitator.rs` | `PaymentVerifier` trait; `Facilitator` struct verifies Solana signatures |
| `solana.rs` | Solana account verification, ed25519 signature check |
| `solana_rpc.rs` | RPC client for fetching blockhash, latest slot, tx status |
| `solana_types.rs` | Solana-specific types (Pubkey, Signature, etc.) |
| `spl_transfer.rs` | SPL token transfer parsing/verification |
| `escrow/` | Escrow program integration (deposit, claim, refund, PDA, verifier) |
| `fee_payer.rs` | Hot wallet pool for fee payer rotation |
| `nonce_pool.rs` | Durable nonce account pool for transaction ordering |
| `traits.rs` | `PaymentVerifier` trait (chain-agnostic for future EVM) |
| `types.rs` | Payment types (PaymentSignature, PaymentInfo, etc.) |

## Router Crate (Smart Routing)

| Module | Purpose |
|--------|---------|
| `scorer.rs` | 15-dimension scorer (<1μs): code density, reasoning markers, technical terms, etc. → tiers (Simple/Medium/Complex/Reasoning) |
| `profiles.rs` | Routing profiles (eco, auto, premium, free) → model assignments per tier |
| `models.rs` | `ModelRegistry`: loads `config/models.toml`, lists available models |

## Protocol Crate (Wire Types)

| Module | Purpose |
|--------|---------|
| `chat.rs` | `ChatRequest`, `ChatResponse`, `Message`, `Choice` (OpenAI-compatible) |
| `payment.rs` | `PaymentSignature`, `PaymentInfo`, `CostBreakdown`, `PaymentRequired` |
| `cost.rs` | Token-cost calculations, fee breakdown (5% platform fee) |
| `model.rs` | `ModelInfo`, pricing, context window, capabilities |
| `streaming.rs` | SSE stream types (`StreamStart`, `StreamChunk`, `StreamEnd`) |
| `vision.rs` | Vision model types (image URLs, base64) |
| `tools.rs` | Tool/function calling definitions |
| `settlement.rs` | Settlement types (receipt, proof of payment) |
| `constants.rs` | Global constants (PLATFORM_FEE_PERCENT, USDC_DECIMALS, etc.) |

## Frontend (Next.js 16, dashboard/)

| Route | Component | Purpose |
|-------|-----------|---------|
| `/` | `page.tsx` | Public landing page |
| `/dashboard` | `dashboard/page.tsx` | Dashboard root |
| `/dashboard/overview` | `dashboard/overview/page.tsx` | Overview (recent requests) |
| `/dashboard/usage` | `dashboard/usage/page.tsx` | Spend analytics, charts |
| `/dashboard/models` | `dashboard/models/page.tsx` | Available models, pricing, info |
| `/dashboard/wallet` | `dashboard/wallet/page.tsx` | Wallet balances, transaction history |
| `/dashboard/settings` | `dashboard/settings/page.tsx` | Org/team settings, API keys, budget limits |
| `/metrics` | `metrics/page.tsx` | Public metrics, system stats |
| `/sponsor` | `sponsor/page.tsx` | Sponsors / backers page |
| `/docs/[[...slug]]` | `docs/[[...slug]]/page.tsx` | Markdown documentation (Fumadocs) |

Key files:
- `lib/api.ts` — Flat async helpers (`fetchHealth`, `fetchModels`, `fetchOrgs`, `createApiKey`, etc.)
- `lib/auth.ts` — API-key getter/setter helpers backed by `localStorage`
- `lib/mock-data.ts` — Mock data for dev (when API unavailable)
- `lib/theme-config.ts` — Design tokens
- `lib/metrics-aggregator.ts` — Aggregates usage data (hourly, daily, monthly)
- `src/proxy.ts` — Server-side gateway proxy helper

## SDKs

| SDK | Language | Purpose | Files |
|-----|----------|---------|-------|
| `typescript/` | TypeScript/Node | Client SDK for agents | chat client, wallet integration |
| `python/` | Python | Python agent SDK | same as TS |
| `go/` | Go | Go agent SDK | same as TS |
| `ai-sdk-provider/` | TypeScript | Vercel AI SDK provider | integrates Solvela with Vercel AI SDK |
| `openclaw-provider/` | TypeScript | OpenClaw provider | OpenClaw AI framework integration |
| `signer-core/` | TypeScript | Transaction signing | Solana tx signing utilities |
| `mcp/` | TypeScript | Claude MCP server | Claude desktop integration |

## Configuration Files

| File | Purpose | Format |
|------|---------|--------|
| `config/default.toml` | Server defaults | TOML: host, port, Solana RPC URL, log level |
| `config/models.toml` | Model registry + pricing | TOML: 44+ models, per-token costs, context windows |
| `config/services.toml` | Service marketplace | TOML: external LLM services |
| `.env.example` | Required env vars | Bash: API keys, wallet keys, database URL |
| `Cargo.toml` | Workspace manifest | TOML: workspace members, shared deps |

## Key Dependencies

| Crate | Version | Purpose |
|-------|---------|---------|
| `axum` | 0.8 | Web framework |
| `tokio` | 1 (full) | Async runtime |
| `serde` + `serde_json` | 1 | Serialization |
| `sqlx` | 0.8 | PostgreSQL driver (optional) |
| `redis` | 1.2 | Cache layer (optional) |
| `reqwest` | 0.12 | HTTP client |
| `tracing` | 0.1 | Structured logging |
| `metrics` + `metrics-exporter-prometheus` | 0.24/0.18 | Metrics export |
| `ed25519-dalek` + `curve25519-dalek` | 2/4 | Solana sig verification |
| `thiserror` | 2 | Error macro (libraries) |
| `anyhow` | 1 | Error context (binaries) |
| `tower` + `tower-http` | 0.5/0.6 | Middleware layers |

## Environment Variables (SOLVELA_ prefix, RCR_ deprecated fallback)

Double-underscore (Fly.io convention) is the canonical separator; single-underscore form is also accepted as a fallback. Legacy `RCR_` prefix still works with a deprecation warning.

| Variable | Required | Purpose | Example |
|----------|----------|---------|---------|
| `SOLVELA_HOST` | No (default 0.0.0.0) | Listen address | 127.0.0.1 |
| `SOLVELA_PORT` | No (default 8402) | Listen port | 8080 |
| `SOLVELA_SOLANA__RPC_URL` | Yes | Solana RPC endpoint | https://api.mainnet-beta.solana.com |
| `SOLVELA_SOLANA__RECIPIENT_WALLET` | Yes | USDC recipient | Hpq... (wallet addr) |
| `SOLVELA_SOLANA__USDC_MINT` | No (defaults to mainnet USDC) | USDC mint | EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v |
| `SOLVELA_SOLANA__FEE_PAYER_KEY` | No | Fee payer hot wallet | base64/JSON private key |
| `SOLVELA_SOLANA__ESCROW_PROGRAM_ID` | No | Escrow program ID | 9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU |
| `DATABASE_URL` | No | PostgreSQL conn (optional) | postgres://user:pass@localhost/solvela |
| `REDIS_URL` | No | Redis conn (optional) | redis://localhost:6379 |
| `OPENAI_API_KEY` | No | OpenAI API key | sk-... |
| `ANTHROPIC_API_KEY` | No | Anthropic API key | sk-ant-... |
| `GOOGLE_API_KEY` | No | Google Gemini key | ... |
| `XAI_API_KEY` | No | xAI Grok key | xai-... |
| `DEEPSEEK_API_KEY` | No | DeepSeek key | sk-... |
| `SOLVELA_DEV_BYPASS_PAYMENT` | No | Skip payment (dev only) | true |
| `SOLVELA_ADMIN_TOKEN` | No | Admin endpoints access token | secret |
| `RUST_LOG` | No (default gateway=info) | Log level filter | debug,tower_http=debug |

## Integration Test Setup (No Live Server)

Tests use `tower::ServiceExt::oneshot` to invoke the router directly:

```rust
#[tokio::test]
async fn test_chat_endpoint() {
    let app = test_app();  // in-memory state
    let request = Request::builder().method("POST").uri("/v1/chat/completions")...;
    let response = app.oneshot(request).await?;
    assert_eq!(response.status(), StatusCode::OK);
}
```

No need for a running server; all tests are isolated and fast.

## Deployment

- **Dockerfile** — 2-stage build (`rust:1.88-slim-trixie` builder → `debian:trixie-slim` runtime, non-root `solvela` user); binary at `/app/solvela-gateway`
- **fly.toml** — Fly.io config; port 8402, region ord (Chicago)
- **docker-compose.yml** — Local dev: PostgreSQL 16, Redis 7
- **Dashboard** — Next.js on Vercel (`solvela.vercel.app`)

Migrations run automatically on startup (idempotent `CREATE IF NOT EXISTS`).

## Related Documentation

- [Backend Routes & Handlers](backend.md) — Detailed route mapping
- [Frontend Components](frontend.md) — Dashboard page tree
- [Database Schema](data.md) — Table relationships
- [External Dependencies](dependencies.md) — Provider integrations
- [STATUS.md](../../STATUS.md) — Live shipping status
- [CLAUDE.md](../../CLAUDE.md) — Development guidelines
