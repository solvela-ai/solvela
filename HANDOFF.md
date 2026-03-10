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

## IMMEDIATE NEXT STEP: Phase E — Smart Features

**Phase D (CLI Tool) is COMPLETE.** All 12 tasks done, 74 tests passing, clippy clean.

**Next up:** Phase E: Smart Features (sessions, cache, degraded detection, free fallback).

**Alternatively:** Remaining RustyClawRouter phases (8, 9, 12, 13, 14) or Phase F (SDKs).

---

## Current Progress

### What's Complete in RustyClawRouter

**Phases 1-7, 10-11, Phase A** — all complete (304+ tests).

**Security audit completed this session** — 6 critical/high findings fixed:
- LRU cache replaces HashSet for replay protection (no full-clear gap)
- 50KB size limit on PAYMENT-SIGNATURE header
- Rate limit cleanup 60-second cooldown
- Session secret length validation (>= 32 bytes)

### What's Complete in RustyClawClient

**Phase B: Core Client Library** — ✅ complete (51 tests)
**Phase C: Proxy Sidecar** — ✅ complete (58 tests total, clippy clean)
**Phase D: CLI Tool (`rcc`)** — ✅ complete (74 tests total, clippy clean)

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
└── client.rs    — RustyClawClient with chat(), chat_stream(), models(), estimate_cost(), sign_payment_for_402(), usdc_balance(), usdc_balance_of()
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
| E: Smart Features | Sessions, cache, degraded detection, free fallback | Not started |
| F: SDKs | Python, TS, Go client SDKs | Not started |
| G: Gateway Changes | Debug headers, session ID, stats endpoint | G.3 (heartbeat) ✅, rest not started |

**Remaining RustyClawRouter phases:** 8, 9, 12, 13, 14 (see ecosystem plan)

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

## What Didn't Work (Cumulative)

- **Concurrent agents modifying same files** — never dispatch parallel agents that touch same files.
- **`git add -A` in subagents** — use specific `git add` paths.
- **`solana-client` dep with `solana-sdk 2.2`** — transitive zeroize conflicts.
- **`spl-associated-token-account` dep** — same conflict chain.
- **`zeroize` derive feature** — conflicts with `curve25519-dalek v3`.
- **`expect()` in library code** — violated project conventions; return `Result` instead.
- **RustyClawClient HTTPS remote** — auth failed; switched to SSH.

---

## Test Commands

```bash
# RustyClawRouter (~/projects/RustyClawRouter/)
cargo test                            # 304+ tests
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings

# RustyClawClient (~/projects/RustyClawClient/)
cd ~/projects/RustyClawClient
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test  # 74 tests (47 unit + 9 client integration + 5 cli-args + 6 cli + 7 proxy integration)
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo clippy --all-targets --all-features -- -D warnings
```

## User Preferences

- Prefers **subagent-driven execution** (option 1) over parallel sessions
- Wants **brainstorming before implementation** (one question at a time)
- Building for **production** use on trading platform + telsi.ai
- Prefers **adaptive/smart defaults** with user overrides
- Rust edition 2021, MSRV 1.85, clippy pedantic
