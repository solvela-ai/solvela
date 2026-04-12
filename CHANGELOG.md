# Changelog

All notable changes to Solvela, in reverse chronological order.

## 2026-04-11 — Solvela Rebrand

- **Full rebrand from RustyClawRouter to Solvela**: renamed all workspace crates (`solvela-protocol`, `solvela-router`, `solvela-gateway`, `solvela-cli`), CLI binary (`solvela`), Fly.io app (`solvela-gateway`), Dockerfile binary target, SDK packages (`solvela-sdk`, `@solvela/sdk`, `@solvela/mcp-server`)
- **Documentation site** (`rcr-docs-site`): Next.js 16 + Fumadocs MDX, 18 pages (Getting Started, Core Concepts, API Reference, SDK Guides, Operations). Deployed to Vercel as `solvela-docs` at `docs.solvela.ai`. Shared theme library (`@rustyclaw/docs-theme`) with `createPresetCSS()` and `createThemeConfig()`. In-repo mdBook at `docs/book/` also rebranded.
- Added `SOLVELA_*` env var prefix with `RCR_*` backward compatibility and deprecation warnings
- Added `X-Solvela-*` HTTP headers with `X-RCR-*` backward compatibility
- Updated Prometheus metrics to `solvela_*` prefix
- Renamed API key prefix from `rcr_k_` to `solvela_k_`
- Updated all documentation (README, HANDOFF.md, CHANGELOG.md) for Solvela branding

## 2026-04-10 — CLI Load Test Framework + Go SDK Real Signing

- **Go SDK real Solana signing**: Added crypto primitives (PDA, ATA, discriminator derivation), TransferChecked tx builder, Anchor escrow deposit tx builder, wallet signing methods, externally-anchored KATs. Replaced stub `createPaymentHeader` with real signing dispatch.
- **CLI load test framework** (`solvela loadtest`): Constant-arrival-rate dispatcher with backpressure tracking, latency histogram metrics collector, load test worker with 402 dance and `PaymentStrategy` trait, terminal + JSON report formatters, ExactPayment strategy (real SPL TransferChecked), EscrowPayment strategy (Anchor deposit), Prometheus scraper + SLO validation, integration tests.

## 2026-04-09 — Escrow Hardening + CLI Recovery

- **Escrow payment scheme**: End-to-end escrow support for CLI and all SDKs (#9)
- **Escrow fixes**: Verifier now parses Anchor deposit instead of SPL transfer (#10), `settle_payment` submits tx + polls for confirmation (#11), corrected ATA derivation in escrow PDA helper (#12), capped escrow claim at `client_amount` when verified deposit unknown (#16)
- **CLI `recover` subcommand**: Refunds expired escrow PDAs with atomic + decimal USDC display (#13)
- **CLI `chat --scheme`**: Added `--scheme` flag to select exact vs escrow payment (#15)
- Externally-anchored escrow PDA regression test (#14)

## 2026-04-08 — Escrow Mainnet Deployment

- **Deployed escrow program to Solana mainnet** (#8). Program ID: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`

## 2026-04-07 — First Real Payment + Production Fixes

- **First real Solana payment processed** on mainnet
- Fixed: use path-only resource URL in payment payload (#6)
- Fixed: wire environment variables to gateway config (#5)
- API pricing context + session handoff update (#7)
- Real Solana signing, CLI tests, product docs, error hardening (#4)

## 2026-04-06 — CLI Tests + PR Review

- CLI test suite (25+ tests across all commands) + PR review findings (#2)

## 2026-04-05 — A2A Protocol Adapter + Enterprise Polish

- **A2A protocol adapter**: `GET /.well-known/agent.json` (AgentCard discovery), `POST /a2a` (JSON-RPC 2.0 dispatcher), `message/send` handler with full x402 payment flow, Redis-backed task state store. 7 commits, 38 new tests.
- **Enterprise follow-up**: `OrgRole` enum replaces `role: String`, split `routes/orgs.rs` (2026 lines) into 7 submodules, wired `RequireOrg`/`RequireOrgAdmin` extractors to all org endpoints, dashboard structured error handling (`ApiResult<T>`).
- Regulatory research: AP2 safe/unsafe boundary documented. Discovery + x402 settlement = safe. Fiat/card processing = requires licensing. Attorney consult scheduled for escrow gray area.

## 2026-04-04 — x402 V2 Migration

- Migrated from x402 V1 to V2 wire format.

## 2026-03-31 — Production Wiring

- Wired Terminal → Solvela gateway (removed direct Anthropic key from rclawterm-gateway)
- Set all 5 provider API keys on Fly.io (Anthropic, xAI, DeepSeek added)
- Funded fee-payer wallet (0.09 SOL)
- Redeployed gateway with Dockerfile fix (`crates/common/` → `crates/protocol/`)
- LiteSVM integration tests: 14 tests for escrow program (5 happy path + 9 error)
- Installed Anchor CLI 0.31.1 + Solana toolchain 3.1.12

## 2026-03-29 — Dashboard API Integration

- All dashboard pages fetch real data from gateway API
- Admin aggregate stats endpoint (`GET /v1/admin/stats`)
- Graceful mock-data fallback with warning banner
- Admin auth via `GATEWAY_ADMIN_KEY` env var (server-side only)
- 82 dashboard tests passing

## 2026-03-18 — Terminal Backend Deploy

- `rclawterm-gateway.fly.dev` deployed, 2 machines (ord)
- Shared Upstash Redis instance (`rustyclawrouter-cache`, later renamed to `solvela-cache`)
- OpenAI + Google API keys set on Fly.io

## Earlier — Core Gateway (Phases 1-4, 8-9, 12-14)

- **Phases 1-3**: Axum HTTP server, x402 middleware, 5 LLM providers, 15-dimension smart router, 4 routing profiles, Redis cache, circuit breaker, Python/TS/Go/MCP SDKs, CLI
- **Phase 4**: Anchor escrow program (deposit/claim/refund, PDA vault, timeout refunds)
- **Phase 8**: Escrow hardening (claim queue, claim processor, fee payer pool rotation, durable nonces, circuit breaker, exponential backoff, escrow metrics)
- **Phase 9**: Service marketplace (external service proxy, admin registration, health checker)
- **Phase 12**: Prometheus monitoring (15 metrics, `/metrics` endpoint)
- **Phase 13**: Documentation overhaul
- **Phase 14**: Production hardening (CatchPanicLayer, timeouts, connection limits, graceful shutdown)
- **Gateway extras**: Debug headers, stats endpoint, session tracking, SSE heartbeat, nonce endpoint
- **Security audits**: Multiple rounds — 7 CRITICAL, 7 HIGH, 4 HIGH, 12 MEDIUM — all resolved
- **Chat route refactor**: Monolithic `chat.rs` (2405 lines) → `chat/` module directory
- Note: crates were originally named `rcr-router`/`rcr-protocol`; now `solvela-router`/`solvela-protocol` following the Apr 11 rebrand
