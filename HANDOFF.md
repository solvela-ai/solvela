# HANDOFF.md — RustyClaw Ecosystem Continuation Guide

> **Start here.** This document captures the full context so a fresh agent can continue without ramp-up time.

---

## Goal

Build the **RustyClaw ecosystem** — a self-sovereign, Solana-native AI agent payment stack:

1. **RustyClawRouter** (gateway server) — verifies x402 payments, routes to LLM providers, settles on Solana. **This repo.**
2. **RustyClawClient** (client library + proxy) — holds wallet, signs payments, makes LLM calls transparent. **Repo:** `~/projects/RustyClawClient/` ([GitHub](https://github.com/sky64/RustyClawClient))
3. **rustyclaw-protocol** (shared wire format) — x402 + chat types used by both. **✅ Complete.** Lives in `crates/protocol/`.

Kenneth is building this for his **trading platform** and **AI assistant platform** (telsi.ai).

**Production gateway target:** Fly.io at `https://rustyclawrouter-gateway.fly.dev` (fly.toml + Dockerfile already configured, account set up)

---

## Status: ALL PHASES COMPLETE

**RustyClawRouter is production-ready.** All 14 phases are complete. No remaining phases.

---

## Current Progress

### What's Complete in RustyClawRouter

**Phases 1-14, Phase A, Phase G** — all complete (373 gateway tests (264 unit + 109 integration), 503 total workspace).

**Phase 14 (Production Hardening) delivered:**
- Safety layers — CatchPanicLayer (returns 500 JSON on panic), TimeoutLayer (120s configurable), ConcurrencyLimitLayer (256 configurable)
- Graceful shutdown — Balance monitor and rate limiter cleanup now respond to shutdown signal via watch channel. All 4 background tasks shut down cleanly.
- Readiness health check — `/health` now returns dependency status (DB, Redis, providers, Solana RPC) with ok/degraded/error status logic
- JSON structured logging — `RCR_LOG_FORMAT=json` env var for production log aggregation
- PostgreSQL pool — Configurable max_connections (default 20) via `RCR_DB_MAX_CONNECTIONS`, 5s acquire timeout
- Shared HTTP client — All 5 provider adapters share a single reqwest::Client from AppState, 90s per-request timeout for LLM calls
- Max message validation — 256 message limit per chat request
- Provider retry — Exponential backoff (2 retries, 1s/2s) for transient failures (timeouts, 5xx). Never retries 4xx. Streaming paths not retried.
- New files: `docs/plans/phase-14-production-hardening.md`
- Modified: 15 files (lib.rs, main.rs, balance_monitor.rs, cache.rs, health.rs, chat.rs, all 5 providers, providers/mod.rs, integration.rs, Cargo.toml/lock)
- 337 gateway tests (235 unit + 102 integration) + 79 x402 tests = 416+ total

**Phase 13 (Documentation) delivered:**
- README.md overhaul — branded shields.io badges (#F97316 orange), Mermaid architecture diagram (dark theme), x402 payment flow sequence diagram, complete 27-model pricing table, 12 API endpoints, SDK examples, CLI overview, project structure
- mdBook documentation site (`docs/book/`) — 22 chapters, 3,102 lines across 4 sections:
  - Getting Started: installation, quickstart (full 402 walkthrough), configuration (complete env var + model pricing tables)
  - Core Concepts: how-it-works (request flow diagram), x402 protocol (deep dive), smart routing (15 dimensions), escrow (deposit/claim/refund flow)
  - API Reference: chat completions, models, services, wallet stats, health/metrics — all with request/response JSON shapes
  - SDK Guides: Python, TypeScript, Go, MCP — complete usage examples
  - Operations: deployment (Docker/Fly.io), monitoring (all 15 Prometheus metrics), security model, troubleshooting (17 common issues)
- SDK READMEs — TypeScript (LLMClient API, OpenAI compat), Go (functional options, typed errors), MCP (Claude Code/Desktop setup, 5 tools)
- Brand-aligned — dark theme Mermaid diagrams with #F97316 orange accent, zero emoji, Telsi.ai aesthetic, Phosphor-compatible monochrome
- New files: 22 mdBook chapters + book.toml, 3 SDK READMEs
- Modified: README.md

**Phase 12 (Prometheus Monitoring) delivered:**
- Prometheus `/metrics` endpoint — admin-gated via `RCR_ADMIN_TOKEN`, renders Prometheus text exposition format
- Request metrics middleware — Tower layer recording `rcr_requests_total` (counter), `rcr_request_duration_seconds` (histogram), `rcr_active_requests` (gauge) with method/path/status labels. Skips `/metrics` to avoid feedback loops. Uses `MatchedPath` for route normalization.
- Payment metrics — `rcr_payments_total` (counter, status: verified/cached/free/none/failed), `rcr_payment_amount_usdc` (histogram), `rcr_replay_rejections_total` (counter) in chat.rs and proxy.rs
- Provider metrics — `rcr_provider_request_duration_seconds` (histogram, provider label), `rcr_provider_errors_total` (counter, provider+error_type labels)
- Cache metrics — `rcr_cache_total` (counter, result: hit/miss/skip)
- Escrow metrics — `rcr_escrow_claims_total` (counter, result: success/failure), `rcr_escrow_queue_depth` (gauge)
- Infrastructure gauges — `rcr_fee_payer_balance_sol` (gauge, pubkey label), `rcr_service_health` (gauge, service_id label)
- New files: `crates/gateway/src/middleware/metrics.rs`, `crates/gateway/src/routes/metrics.rs`, `docs/plans/phase-12-monitoring.md`
- Modified: workspace + gateway + x402 Cargo.toml, `lib.rs` (PrometheusHandle in AppState, /metrics route, metrics middleware), `main.rs` (recorder init), `chat.rs`/`proxy.rs` (payment/provider/cache metrics), `claim_processor.rs` (escrow metrics), `balance_monitor.rs` (fee payer gauge), `service_health.rs` (health gauge)
- 7 new integration tests for metrics endpoint and instrumentation

**Phase 9 (Service Marketplace) delivered:**
- Service proxy endpoint (`POST /v1/services/{service_id}/proxy`) — accepts arbitrary JSON, verifies x402 payments with 5% platform fee, forwards to external services with 60s timeout
- Service registration (`POST /v1/services/register`) — admin-auth (`RCR_ADMIN_TOKEN`), validates ID format/HTTPS/uniqueness, returns 201
- Health monitoring — background checker runs every 60s (configurable via `RCR_SERVICE_HEALTH_INTERVAL_SECS`), concurrent HEAD probes, graceful shutdown via watch channel
- Service discovery — `GET /v1/services` now includes health status and supports filtering
- Security hardening — SSRF prevention (private network filtering at registration and proxy time), constant-time admin token comparison, integer USDC arithmetic, proper payer wallet extraction
- `ServiceRegistry` refactored with `RwLock` for async access, new fields: `source`, `healthy`, `price_per_request_usdc`
- New files: `routes/proxy.rs`, `service_health.rs`, `security.rs`, `docs/plans/phase-9-service-marketplace.md`
- 12 new integration tests covering proxy, registration, health, and security

**Phase G delivered (final pass):**
- G.2: Debug response headers — RequestId middleware (always-on `X-RCR-Request-Id`), 11 debug headers opt-in via `X-RCR-Debug: true`. CORS `expose_headers` added for browser clients. 6 new integration tests for CORS header exposure.
- G.5: Stats endpoint — `GET /v1/wallet/{address}/stats` with session auth (`x-rcr-session` HMAC), 3 DB queries (summary, by_model, by_day). Query functions moved to `usage.rs` (`get_wallet_stats`, `get_stats_by_model`, `get_stats_by_day`). Wallet mismatch enforcement (token wallet != path wallet -> 403). Response shape tests added.
- G.1: Session ID echo + `SpendLogEntry` refactor + migration `003_phase_g_request_session_ids.sql` (request_id + session_id columns). 8 unit tests for validation and attachment. 2 migration validation tests.
- G.3: SSE heartbeat (completed earlier)
- G.4: Nonce endpoint (completed earlier)
- Most functionality was already implemented in prior phases; Phase G final pass filled gaps (CORS headers, tests, query refactoring)
- 373 gateway tests (264 unit + 109 integration), 503 total workspace

**Security audit completed (pre-Phase G final pass):**
- 7 CRITICAL fixed: payment amount parsing, budget defaults, devnet fallback, stub-to-paid rejection, stats wallet matching, amount bypass vulnerability, mandatory replay protection
- 7 HIGH fixed: Redis logging, durable nonce TTL, Anthropic streaming, f64 epsilon, CI action pinning, quinn-proto update, ConnectInfo extraction
- 4 HIGH fixed (earlier): field validation, optimistic settlement guard, max_tokens cap
- Multiple MEDIUM fixed: CORS tightening, error message leakage, USDC mint enforcement, Docker port exposure, and others
- Earlier audit (Phase B): LRU cache replaces HashSet for replay protection, 50KB PAYMENT-SIGNATURE limit, rate limit cleanup cooldown, session secret length validation

**Phase 8 (Escrow Hardening) delivered:**
- 8.1: Claim processor auto-start on gateway boot (gated on escrow + DB)
- 8.2: Fee payer rotation with health tracking and 60s cooldown
- 8.3: Durable nonces for claim transactions (blockhash fallback)
- 8.4: Exponential backoff (1s-5min), max 10 retries, circuit breaker
- 8.5: GET /v1/escrow/config endpoint (program ID, current slot, USDC mint)
- 8.6: GET /v1/escrow/health endpoint (admin-auth gated, claim metrics)
- 8.7: Integration tests for escrow endpoints and scheme validation
- Graceful shutdown via watch channel, stale claim recovery after 5min
- Code review fixes: deadlock fix, keypair zeroing, shared HTTP client

### What's Complete in RustyClawClient

**Phase B: Core Client Library** — ✅ complete (51 tests)
**Phase C: Proxy Sidecar** — ✅ complete (58 tests total, clippy clean)
**Phase D: CLI Tool (`rcc`)** — ✅ complete (74 tests total, clippy clean)

**Phase E delivered:**
- `ResponseCache` — `Mutex<LruCache>`, 200 entries, 10min TTL, 30s dedup window
- `SessionStore` — `RwLock<HashMap>`, configurable TTL, three-strike escalation
- `BalanceMonitor` — opt-in background poller, `Arc<AtomicU64>` shared state, low-balance callback with transition debouncing
- Degraded response detection — `EmptyContent`, `RepetitiveLoop`, `TruncatedMidWord`, `KnownErrorPhrase`
- Free tier fallback — auto-swap model when wallet balance is zero
- Smart `chat()` flow — cache check → balance guard → session lookup → send → degraded retry → cache store + session update
- Smart `chat_stream()` flow — balance guard + session lookup (cache/degraded skipped for streaming)
- All features opt-in via `ClientBuilder` flags (default: off)
- Cache key computed after model finalization (prevents cross-model pollution)

**Phase D delivered:**
- `rcc` CLI binary — 4 commands: `wallet`, `chat`, `models`, `doctor`
- `rustyclawclient-cli-args` shared crate — `WalletArgs`, `GatewayArgs`, `RpcArgs` + `load_wallet()`/`save_wallet()` (used by both proxy and CLI)
- `wallet create` — BIP39 mnemonic generation, Solana CLI byte-array format, `0o600` permissions
- `wallet import` — `--mnemonic` or `--keypair` (base58), with `--force` overwrite
- `wallet balance` — USDC-SPL balance via JSON-RPC (`getTokenAccountBalance`)
- `wallet address` — Print public key
- `wallet export` — Print base58 keypair with confirmation prompt
- `chat` — Streaming by default (SSE via `reqwest-eventsource`), `--no-stream`, TTY-aware output (stream in TTY, JSON when piped), cost info to stderr
- `models` — Table format in TTY, JSON when piped, `--provider` filter, `--json` flag
- `doctor` — 6 sequential checks: wallet, gateway, models, RPC, balance, payment flow (pass/fail/warn/skip)
- `chat_stream()` on `RustyClawClient` — returns `impl Stream<Item = Result<ChatChunk, ClientError>>`
- `usdc_balance()` / `usdc_balance_of()` on `RustyClawClient` — query USDC-SPL balance via JSON-RPC
- `Wallet::to_keypair_bytes()` / `to_keypair_b58()` — export methods for wallet persistence
- Proxy refactored to use shared `rustyclawclient-cli-args` (75 lines deleted, 16 added)

**Crate structure:**
```
crates/rustyclaw-client/src/
├── lib.rs       — Module declarations + pub use re-exports
├── error.rs     — WalletError, SignerError, ClientError, BalanceError (thiserror)
├── config.rs    — ClientConfig + ClientBuilder
├── wallet.rs    — Wallet (Keypair, BIP39, from_keypair_bytes, to_keypair_bytes/b58, zeroize)
├── signer.rs    — sign_exact_payment, build_payment_payload, encode_payment_header, associated_token_address (pub(crate))
├── client.rs    — RustyClawClient with chat(), chat_stream(), models(), estimate_cost(), sign_payment_for_402(), usdc_balance(), usdc_balance_of(), last_known_balance(), balance_state()
├── cache.rs     — ResponseCache (Mutex<LruCache>, TTL, dedup window) (pub(crate))
├── session.rs   — SessionStore (RwLock<HashMap>, TTL, three-strike) (pub(crate))
├── quality.rs   — is_degraded() + DegradedReason enum (pub(crate))
└── balance.rs   — BalanceMonitor (pub) — opt-in background poller with Arc<AtomicU64>
tests/
└── integration.rs — 9 wiremock-based tests

crates/rustyclawclient-cli-args/src/
└── lib.rs       — WalletArgs, GatewayArgs, RpcArgs (clap Args), load_wallet(), save_wallet(), expand_home()

crates/rustyclawclient-cli/src/
├── main.rs      — Cli struct, Commands enum, dispatch
└── commands/
    ├── wallet.rs  — create, import, balance, address, export
    ├── chat.rs    — streaming chat with TTY detection
    ├── models.rs  — table/JSON output with provider filter
    └── doctor.rs  — 6 diagnostic checks

crates/rustyclawclient-proxy/src/
├── lib.rs       — Re-exports ProxyState + build_proxy_router
├── main.rs      — CLI (clap) + shared args + server startup
└── proxy.rs     — Catch-all handler, 402 interception, streaming passthrough
tests/
└── integration.rs — 7 wiremock + tower::oneshot tests
```

### Phase C Design Decisions (from brainstorming)

| # | Decision | Rationale |
|---|----------|-----------|
| 26 | Catch-all reverse proxy (not endpoint-specific) | Simpler, works for all endpoints, no body parsing on non-402 |
| 27 | Axum for proxy server | Already in workspace, gives routing + body limits + shutdown |
| 28 | Passthrough streaming | Most SDKs default `stream: true`; passthrough is simplest |
| 29 | Body clone via `Bytes` | Simple, bounded by 10MB limit, needed for 402 retry |
| 30 | Wallet: env var priority, file fallback | Max flexibility; env var standard for CI, file for dev |
| 31 | Wallet file = Solana CLI byte-array format | Compatible with `solana-keygen`, no new format |
| 32 | Bind `127.0.0.1` only | Local proxy must never be network-accessible |
| 33 | Gateway default `https://rustyclawrouter-gateway.fly.dev` | Production-ready; proxy and gateway on different machines |
| 34 | Security flags as CLI args | Easy per-invocation, leverages existing ClientConfig |
| 35 | Strip caller's PAYMENT-SIGNATURE | Prevents injection of fraudulent payment proofs |
| 36 | Structured JSON error responses | Caller can programmatically handle errors |

### Phase E Design Decisions (from brainstorming)

| # | Decision | Rationale |
|---|----------|-----------|
| 47 | `Mutex<LruCache>` for caches, not DashMap | Short critical sections; matches mini-redis, hyper-util patterns |
| 48 | `RwLock<HashMap>` for sessions | Reads dominate writes; matches Solana client pattern |
| 49 | Opt-in `BalanceMonitor`, no auto-spawn | No client library auto-spawns background tasks (researched Solana, ethers-rs, Stripe, AWS) |
| 50 | `Arc<AtomicU64>` shared balance state | Lock-free reads on every request; sentinel `u64::MAX` = unknown |
| 51 | Gateway-driven fallback for degraded responses | Client detects, sends `X-RCR-Retry-Reason: degraded`, gateway picks next model |
| 52 | Free tier fallback via `openai/gpt-oss-120b` | Zero-cost model in gateway config; client swaps when balance is zero |
| 53 | Three-strike escalation on sessions | 3+ identical request hashes in last 10 → `escalated: true` (tracked, not yet acted on) |
| 54 | Cache key after model finalization | Prevents cross-model pollution when balance guard or session changes model |
| 55 | Low-balance callback with transition debouncing | Only fires on not-low→low transition, not every tick |
| 56 | Skip cache/degraded for streaming | Caching streams is complex; degraded detection requires full response |

### Phase D Design Decisions (from brainstorming)

| # | Decision | Rationale |
|---|----------|-----------|
| 37 | Shared `rustyclawclient-cli-args` crate | Follows Solana `clap-utils` / Foundry `foundry-cli` pattern; DRY across proxy + CLI |
| 38 | Balance on `RustyClawClient`, not `Wallet` | Industry standard (Solana SDK, ethers-rs): Wallet = signing, Client = I/O |
| 39 | SSE streaming via `reqwest-eventsource` | 763k downloads/month, used by aichat + async-openai; probe+stream pattern for 402 |
| 40 | TTY-aware output (`std::io::IsTerminal`) | Stable since Rust 1.70; stream in TTY, JSON when piped (follows `mods` pattern) |
| 41 | Table in TTY, JSON when piped for models | Standard CLI UX (gh, kubectl, docker); `--json` flag for explicit override |
| 42 | Binary name `rcc` | Short, memorable, follows `rcr` (router) naming convention |
| 43 | Solana CLI wallet format (JSON byte array) | Compatible with `solana-keygen`, no custom format, industry standard |
| 44 | Mnemonic shown once, never stored | Security best practice (solana-keygen, cast wallet new) |
| 45 | `0o600` file permissions on wallet | Unix security standard for sensitive files |
| 46 | Doctor command with 6 sequential checks | Follows `rustyclawrouter doctor`, progressive diagnostics with skip on failure |

---

### Ecosystem Phase Summary

**RustyClawClient ecosystem** (master plan: `.claude/plan/rustyclaw-ecosystem.md`):
| Phase | Description | Status |
|-------|-------------|--------|
| A: Extract Protocol Crate | `rustyclaw-protocol` | ✅ Complete |
| B: Core Client Library | Wallet, signer, client | ✅ Complete |
| C: Proxy Sidecar | localhost OpenAI-compat proxy | ✅ Complete (58 tests) |
| D: CLI (`rcc`) | wallet, chat, models, doctor commands | ✅ Complete (74 tests) |
| E: Smart Features | Sessions, cache, degraded detection, free fallback | ✅ Complete (121 tests) |
| F: SDKs | Python, TS, Go client SDKs | ✅ Complete (301 tests across 3 repos) |
| G: Gateway Changes | Debug headers, session ID, stats endpoint | ✅ Complete (G.1-G.5 all done, 373 gateway / 503 workspace tests) |
| 8: Escrow Hardening | Claim recovery, fee payer rotation, monitoring | ✅ Complete (384 tests) |
| 9: Service Marketplace | Proxy, registration, health, SSRF prevention | ✅ Complete (308 gateway tests) |
| 12: Monitoring | Prometheus metrics, request/payment/provider/cache/escrow instrumentation | ✅ Complete (316 gateway + 79 x402 tests) |
| 13: Documentation | README overhaul, mdBook site (22 chapters), SDK READMEs, brand alignment | ✅ Complete |
| 14: Production Hardening | Safety layers, graceful shutdown, readiness health, JSON logging, shared HTTP client, provider retry | ✅ Complete (337 gateway + 79 x402 = 416+ tests) |

**All RustyClawRouter phases complete. Production-ready.**

---

## Key Dependency Workarounds

- `solana-client` and `spl-associated-token-account` conflict with `solana-sdk 2.2` via transitive `zeroize` version incompatibilities
- **Solution:** Manual ATA derivation via `Pubkey::find_program_address`, reqwest JSON-RPC for blockhash fetching
- `zeroize` must NOT have `derive` feature (conflicts with `curve25519-dalek v3`)
- OpenSSL env vars required on this machine: `OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl`
- RustyClawClient remote uses SSH: `git@github.com:sky64/RustyClawClient.git`

---

## What Worked (Cumulative)

- **Subagent-Driven Development** — fresh subagent per task with two-stage review. Kenneth's preferred approach.
- **Brainstorming skill before implementation** — one-question-at-a-time design.
- **Parallel subagent dispatch** — safe when tasks modify different files.
- **Security audit before Phase C** — caught 6 critical/high issues across both repos.
- **Manual ATA derivation** — cleaner than `spl-associated-token-account` with dep conflicts.
- **reqwest JSON-RPC for blockhash** — avoids `solana-client` entirely.
- **Two-stage code review (spec then quality)** — caught 7 hardening issues in Phase C proxy.
- **Extracting `sign_payment_for_402()`** — decoupled signing from ChatRequest/ChatResponse, enabling proxy reuse.
- **Shared CLI args crate** — `rustyclawclient-cli-args` eliminated duplication between proxy and CLI binaries.
- **Industry research before design decisions** — parallel research agents for each decision (CLI patterns, streaming, wallet formats, balance APIs).
- **Probe+stream pattern for 402 payment** — non-streaming probe to get 402, then stream with payment header. Clean SSE integration.
- **TTY-aware output** — `std::io::IsTerminal` for smart defaults (stream/table in TTY, JSON when piped).

**Phase E commit history (RustyClawClient):**
```
a413f39 feat: integrate balance guard and session lookup into chat_stream()
e0107c7 fix: compute cache key after model finalization to prevent cross-model pollution
a8143f4 feat: integrate smart features into chat() request flow
cf38659 refactor: address review feedback on BalanceMonitor
65f46d5 feat: add opt-in BalanceMonitor with polling and low-balance callback
2d7741a feat: add shared balance state and optional smart feature stores to RustyClawClient
a34009d feat: add smart feature config fields to ClientConfig and ClientBuilder
d1f06ba feat: add degraded response detection with DegradedReason enum
02c49c7 feat: add SessionStore with TTL and three-strike escalation
03fdaca feat: add ResponseCache with LRU eviction and TTL
0a368cd chore: add lru crate to workspace dependencies
```

**Phase D commit history (RustyClawClient):**
```
eedac45 feat: implement doctor command with 6 diagnostic checks
c2bfe58 feat: implement chat command with streaming, TTY detection, and JSON pipe output
bde7809 feat: implement models command with table/JSON output and provider filter
2f76325 feat: implement wallet create, import, balance, address, export commands
693cb04 feat: scaffold rcc CLI binary with command stubs
e3d57db refactor: proxy uses shared cli-args crate for wallet loading and common args
43874cb feat: add rustyclawclient-cli-args shared crate with WalletArgs, GatewayArgs, load_wallet
4ce1b50 feat: add chat_stream() with SSE support to RustyClawClient
1fe646e feat: add usdc_balance() and usdc_balance_of() to RustyClawClient
448d009 chore: add reqwest-eventsource to workspace dependencies
```

**Phase C commit history (RustyClawClient):**
```
92af973 fix: address code review findings for proxy hardening
c385253 refactor: move reqwest::Client to ProxyState and extract forward_headers helper
9aaeb08 docs: add Phase C proxy sidecar implementation plan
fc4b0d6 test: add integration tests for proxy handler
abbbd2c feat: implement catch-all proxy handler with 402 interception
4d2d737 feat: add CLI args, wallet loading, and server startup for proxy
6a84c4a chore: scaffold rustyclawclient-proxy binary crate
692517c feat: add sign_payment_for_402() and Wallet::from_keypair_bytes()
```

**Phase F delivered (3 separate repos, fresh implementations from Rust client reference):**
- **`rustyclaw-python`** (`~/projects/rustyclaw-python/`) — 114 tests, Python 3.10+, httpx + solders
- **`rustyclaw-ts`** (`~/projects/rustyclaw-ts/`) — 100 tests, Node 18+, @solana/web3.js + native fetch
- **`rustyclaw-go`** (`~/projects/rustyclaw-go/`) — 87 tests, Go 1.21+, stdlib net/http + ed25519

Each SDK implements:
- Wire-format types (matching `rustyclaw-protocol` exactly, snake_case JSON)
- Wallet with keypair management + BIP39 mnemonics + secret redaction
- Pluggable `Signer` interface with `KeypairSigner` (USDC-SPL transfers)
- LRU response cache with TTL and dedup window
- Session store with three-strike escalation
- Quality check (4 degradation heuristics: empty, error phrases, repetitive loops, truncated)
- HTTP transport with SSE streaming
- Balance monitor with transition-debounced low-balance callback
- 7-step smart chat flow (balance guard → session → cache → send → quality → cache store → session update)
- OpenAI-compatible wrapper (Python/TS only — Go skips, no dominant Go OpenAI SDK)
- Live contract tests (skipped by default)
- CI with multi-version matrix (Python 3.10-3.12, Node 18/20/22, Go 1.21-1.23)

### Phase F Design Decisions (from brainstorming)

| # | Decision | Rationale |
|---|----------|-----------|
| 57 | Separate repos per language | Industry standard, independent release cycles |
| 58 | Fresh start from Rust reference | Clean slate avoids inheriting quirks from old sdks/ code |
| 59 | Full on-chain signing, pluggable signer | Industry standard for crypto SDKs, proxy overkill for micropayments |
| 60 | Minimal dependencies (HTTP + crypto) | Fewer conflicts for consumers |
| 61 | Unit + integration + live contract tests | Maximum confidence, live tests catch real protocol issues |
| 62 | Layered client architecture (1:1 Rust mapping) | Each module testable in isolation, easy cross-referencing |
| 63 | OpenAI compat via wrapper classes (not subclass) | Zero dep on openai package |
| 64 | Cache key after model finalization | Same fix as Rust client, prevents cross-model pollution |
| 65 | Go skips OpenAI compat | No dominant Go OpenAI SDK pattern to mimic |

### Phase G Design Decisions (from brainstorming)

| # | Decision | Rationale |
|---|----------|-----------|
| 66 | Debug headers opt-in via `X-RCR-Debug: true` | No info leakage by default; matches Cloudflare `cf-debug` pattern |
| 67 | 11 debug headers (7 original + request ID + payment status + token estimates) | All data already computed; headers are negligible overhead |
| 68 | Request ID: client-provided with server fallback | Industry standard for distributed tracing; enables end-to-end correlation |
| 69 | Request ID always returned (not gated by debug flag) | Zero security risk (random UUID); massive operational value; matches Stripe/AWS/GitHub |
| 70 | Hybrid: Request ID in middleware, debug headers in handler | Request ID needs early availability; debug data is local to handler |
| 71 | Stats path: `GET /v1/wallet/{address}/stats` | RESTful resource-oriented; self-documenting |
| 72 | Stats time range: `?days=N` (default 30, max 365) | YAGNI; covers 90% of cases |
| 73 | Stats shape: summary + by_model + by_day | Covers CLI, SDK, and dashboard needs |
| 74 | Stats auth: reuse `x-rcr-session` HMAC token | Zero new infrastructure; token already issued and verified |
| 75 | Session ID: echo + log, no server-side sticking | Client handles sticking; server adds cost tracking without duplicating logic |
| 76 | No server-generated session IDs | Sessions are a client concept; Request ID covers per-request tracking |
| 77 | Single migration for request_id + session_id | Both are simple ALTER TABLE; one migration is cleaner |
| 78 | Partial index on session_id (WHERE NOT NULL) | Most rows null initially; partial index avoids bloat |
| 79 | Build order: G.2 → G.5 → G.1 | Debug headers are foundation; stats is more complex; session ID is simplest |

### Phase 8 Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 80 | Keep single-signer constraint (fee_payer == provider) | Avoid Anchor program changes during hardening phase |
| 81 | Exponential backoff 1s-5min, max 10 retries | Industry standard; prevents thundering herd |
| 82 | Circuit breaker: 50% failure in 5min window, 1min pause | Protects RPC from retry storms |
| 83 | Durable nonces for claims with blockhash fallback | Prevents claim expiry on slow networks |
| 84 | GET /v1/escrow/config public, /health admin-gated | Config is discovery; health is operational |
| 85 | In-memory claim metrics (AtomicU64) | No external metrics crate needed |
| 86 | watch channel for graceful shutdown | Clean claim processor exit on SIGTERM |
| 87 | Recover stale in_progress claims after 5min | Prevents permanent stuck claims |
| 88 | Single Mutex for circuit breaker state | Eliminates ABBA deadlock risk |

### Phase 9 Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 89 | SSRF prevention with DNS resolution validation at registration and proxy time | Defense-in-depth; prevents private network access at both layers |
| 90 | Constant-time admin token comparison | Prevents timing attacks on admin authentication |
| 91 | Integer USDC arithmetic via `compute_service_cost_atomic()` | Prevents floating-point divergence in financial calculations |
| 92 | Concurrent health probes with batch lock update | Avoids O(N*timeout) contention; single write lock per cycle |
| 93 | Payer wallet extraction from transaction signer (direct) or `agent_pubkey` (escrow) | Correct attribution for both payment schemes |
| 94 | Service registration validates ID format, HTTPS requirement, uniqueness | Security baseline: no HTTP endpoints, no duplicates, safe IDs |
| 95 | Health probe accepts 2xx, 402, 405 as "healthy" | Services behind x402 return 402 for unauthenticated HEAD; 405 means server is up |

### Phase 12 Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 96 | `metrics` + `metrics-exporter-prometheus` over `prometheus` crate | Idiomatic Rust, macro API, better ergonomics |
| 97 | Hybrid middleware + inline instrumentation | Request metrics cross-cutting (middleware), domain metrics inline (chat/proxy/escrow) |
| 98 | Admin-gated `/metrics` endpoint | Consistent with escrow health pattern; prevents metric data leakage |
| 99 | `rcr_` prefix on all metric names | Matches `RCR_` env var convention; avoids collisions |
| 100 | Global recorder initialized in main.rs, PrometheusHandle in AppState | Single init point; handle passed via Axum state for rendering |
| 101 | Exclude `/metrics` from its own request counter | Avoids Prometheus scraping feedback loop inflating request counts |

### Phase 13 Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 102 | mdBook over Starlight/Docusaurus | Rust-native, single binary, Playground integration, no Node.js toolchain |
| 103 | Mermaid over D2/Excalidraw for inline diagrams | Universal GitHub + mdBook support, zero build step |
| 104 | Dark theme with #F97316 accent | Matching Telsi.ai brand identity |
| 105 | Phosphor Icons (monochrome) as icon system | No colored icons; consistent with brand aesthetic |
| 106 | Shields.io flat badges with brand orange for README header | Visual consistency, professional appearance |

### Phase 14 Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 107 | CatchPanicLayer as outermost middleware | Catches panics before any other layer; returns 500 JSON instead of connection drop |
| 108 | 120s global request timeout (configurable) | Generous for LLM streaming; prevents unbounded hangs |
| 109 | 256 concurrent request limit (configurable) | Prevents resource exhaustion; configurable for scaling |
| 110 | Watch channel shutdown for all background tasks | All 4 background tasks shut down cleanly on SIGTERM |
| 111 | Readiness health check with dependency status | `/health` returns ok/degraded/error based on DB, Redis, providers, Solana RPC |
| 112 | JSON logging via RCR_LOG_FORMAT env var | Production log aggregation without changing code |
| 113 | PgPool max_connections=20 (configurable) | Reasonable default; tunable via `RCR_DB_MAX_CONNECTIONS` |
| 114 | Shared reqwest::Client for all providers | Connection pooling; eliminates 5 separate clients |
| 115 | 90s per-request timeout for LLM calls | Long enough for streaming responses; prevents unbounded waits |
| 116 | Max 256 messages per chat request | Prevents abuse; generous for real conversations |
| 117 | 2 retries with exponential backoff for transient provider failures | 1s/2s delays; never retries 4xx; streaming paths not retried |

### Phase G Final Pass Design Decisions

| # | Decision | Rationale |
|---|----------|-----------|
| 118 | CORS `expose_headers` for all `X-RCR-*` and `X-Session-Id` headers | Browser clients cannot read custom response headers without explicit CORS exposure; required for SDK/dashboard consumers |
| 119 | Stats query functions (`get_wallet_stats`, `get_stats_by_model`, `get_stats_by_day`) in `usage.rs`, not separate `stats.rs` | Collocates all DB query logic in one module; follows existing pattern where `usage.rs` owns all spend_logs queries |

## What Didn't Work (Cumulative)

- **Concurrent agents modifying same files** — never dispatch parallel agents that touch same files.
- **`git add -A` in subagents** — use specific `git add` paths.
- **`solana-client` dep with `solana-sdk 2.2`** — transitive zeroize conflicts.
- **`spl-associated-token-account` dep** — same conflict chain.
- **`zeroize` derive feature** — conflicts with `curve25519-dalek v3`.
- **`expect()` in library code** — violated project conventions; return `Result` instead.
- **RustyClawClient HTTPS remote** — auth failed; switched to SSH.
- **Main agent implementing code directly instead of delegating to subagents** — Kenneth prefers subagent-driven execution. Main agent should brainstorm/plan, then dispatch subagents for implementation. Do NOT implement code in the main conversation.
- **Nested Mutex acquisition in circuit breaker** — caused ABBA deadlock risk; consolidated into single Mutex.

---

## Test Commands

```bash
# RustyClawRouter (~/projects/RustyClawRouter/)
cargo test                            # 373 gateway tests (264 unit + 109 integration), 503 total workspace
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings

# RustyClawClient (~/projects/RustyClawClient/)
cd ~/projects/RustyClawClient
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test  # 121 tests (93 unit + 9 client integration + 5 cli + 6 cli-args + 7 proxy integration + 1 doc-test)
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo clippy --all-targets --all-features -- -D warnings

# SDK Tests (~/projects/rustyclaw-{python,ts,go}/)
cd ~/projects/rustyclaw-python && source .venv/bin/activate && pytest tests/ --ignore=tests/live -q  # 114 tests
cd ~/projects/rustyclaw-ts && npx vitest run  # 100 tests
cd ~/projects/rustyclaw-go && go test ./... -v  # 87 tests
```

## User Preferences

- Prefers **subagent-driven execution** (option 1) over parallel sessions
- Wants **brainstorming before implementation** (one question at a time)
- Building for **production** use on trading platform + telsi.ai
- Prefers **adaptive/smart defaults** with user overrides
- Rust edition 2021, MSRV 1.85, clippy pedantic
