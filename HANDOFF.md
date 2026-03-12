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

## IMMEDIATE NEXT STEP: Remaining RustyClawRouter Phases

**Phase 8 (Escrow Hardening) is COMPLETE.** All sub-tasks (8.1-8.7) implemented and merged to main. 384 tests passing.

**Remaining RustyClawRouter phases:** 9 (Service Marketplace), 12 (Monitoring), 13 (Docs/Examples), 14 (Production Hardening). See ecosystem plan for details.

---

## Current Progress

### What's Complete in RustyClawRouter

**Phases 1-8, 10-11, Phase A, Phase G** — all complete (384 tests).

**Phase G delivered:**
- G.2: Debug response headers — RequestId middleware (always-on `X-RCR-Request-Id`), 11 debug headers opt-in via `X-RCR-Debug: true`
- G.5: Stats endpoint — `GET /v1/wallet/{address}/stats` with session auth (`x-rcr-session` HMAC), 3 DB queries (summary, by_model, by_day)
- G.1: Session ID echo + `SpendLogEntry` refactor + migration `003_phase_g_request_session_ids.sql` (request_id + session_id columns)
- G.3: SSE heartbeat (completed earlier)
- G.4: Nonce endpoint (completed earlier)

**Security audit completed** — comprehensive review with fixes:
- 2 CRITICAL fixed: amount bypass vulnerability, mandatory replay protection
- 4 HIGH fixed: ConnectInfo extraction, field validation, optimistic settlement guard, max_tokens cap
- Multiple MEDIUM fixed: CORS tightening, error message leakage, USDC mint enforcement, Docker port exposure, and others
- Remaining TODO: M3 payer wallet extraction from transaction
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

### What's NOT Done Yet

**RustyClawClient ecosystem** (master plan: `.claude/plan/rustyclaw-ecosystem.md`):
| Phase | Description | Status |
|-------|-------------|--------|
| A: Extract Protocol Crate | `rustyclaw-protocol` | ✅ Complete |
| B: Core Client Library | Wallet, signer, client | ✅ Complete |
| C: Proxy Sidecar | localhost OpenAI-compat proxy | ✅ Complete (58 tests) |
| D: CLI (`rcc`) | wallet, chat, models, doctor commands | ✅ Complete (74 tests) |
| E: Smart Features | Sessions, cache, degraded detection, free fallback | ✅ Complete (121 tests) |
| F: SDKs | Python, TS, Go client SDKs | ✅ Complete (301 tests across 3 repos) |
| G: Gateway Changes | Debug headers, session ID, stats endpoint | ✅ Complete (G.1-G.5 all done, 342 tests) |
| 8: Escrow Hardening | Claim recovery, fee payer rotation, monitoring | ✅ Complete (384 tests) |

**Remaining RustyClawRouter phases:** 9, 12, 13, 14 (see ecosystem plan)

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
cargo test                            # 384 tests
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
