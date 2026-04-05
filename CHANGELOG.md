# Changelog

All notable changes to RustyClawRouter, in reverse chronological order.

## 2026-04-05 — A2A Protocol Adapter + Enterprise Polish

- **A2A protocol adapter**: `GET /.well-known/agent.json` (AgentCard discovery), `POST /a2a` (JSON-RPC 2.0 dispatcher), `message/send` handler with full x402 payment flow, Redis-backed task state store. 7 commits, 38 new tests.
- **Enterprise follow-up**: `OrgRole` enum replaces `role: String`, split `routes/orgs.rs` (2026 lines) into 7 submodules, wired `RequireOrg`/`RequireOrgAdmin` extractors to all org endpoints, dashboard structured error handling (`ApiResult<T>`).
- Regulatory research: AP2 safe/unsafe boundary documented. Discovery + x402 settlement = safe. Fiat/card processing = requires licensing. Attorney consult scheduled for escrow gray area.

## 2026-04-04 — x402 V2 Migration

- Migrated from x402 V1 to V2 wire format.

## 2026-03-31 — Production Wiring

- Wired Terminal → RCR (removed direct Anthropic key from rclawterm-gateway)
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
- Shared Upstash Redis instance (`rustyclawrouter-cache`)
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
