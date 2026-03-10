# HANDOFF.md — RustyClaw Ecosystem Continuation Guide

> **Start here.** This document captures the full context so a fresh agent can continue without ramp-up time.

---

## Goal

Build the **RustyClaw ecosystem** — a self-sovereign, Solana-native AI agent payment stack:

1. **RustyClawRouter** (gateway server) — verifies x402 payments, routes to LLM providers, settles on Solana. **This repo.**
2. **RustyClawClient** (client library) — holds wallet, signs payments, makes LLM calls transparent. **✅ Complete.** Repo: `~/projects/RustyClawClient/` ([GitHub](https://github.com/sky64/RustyClawClient))
3. **rustyclaw-protocol** (shared wire format) — x402 + chat types used by both. **✅ Complete.** Lives in `crates/protocol/`.

Kenneth is building this for his **trading platform** and **AI assistant platform** (telsi.ai).

---

## Current Progress

### What's Complete in RustyClawRouter

**Phases 1-7 of the ecosystem upgrade plan** (`docs/plans/2026-03-09-ecosystem-upgrade-plan.md`):
- Phase 1: Wire format foundation (Developer role, tool calls, vision types, streaming deltas)
- Phase 2: Durable escrow claim queue (PostgreSQL-backed, 5-retry limit)
- Phase 3: Expanded model registry (16 → 27 models, 5 providers)
- Phase 4: Session tokens (HMAC-SHA256, `SessionClaims` with wallet/budget/models/expiry)
- Phase 5: ~~Extracted `x402-solana` crate~~ → Now `rustyclaw-protocol` (see Phase A below)
- Phase 6: ElizaOS plugin (`integrations/elizaos/`)
- Phase 7: Dashboard API stub (`GET /v1/dashboard/spend`)

**Phases 10-11 (SSE Heartbeat + Provider Failover):**
- Adaptive SSE heartbeat stream wrapper (5s → 2s after 10s silence)
- Per-model circuit breaker + model-level fallback chains
- `X-RCR-Fallback` and `X-RCR-Fallback-Preference` headers

**Phase A: rustyclaw-protocol extraction** — ✅ complete:
- Merged payment types (from `x402-solana`) and chat/LLM types (from `rcr-common`) into `crates/protocol/`
- Deleted both `x402-solana` and `rcr-common` crates
- Moved `ServiceRegistry` into `gateway`
- 304 tests passing, clippy clean

### What's Complete in RustyClawClient

**Phase B: Core Client Library** — ✅ complete:
- **Repo:** `~/projects/RustyClawClient/` — https://github.com/sky64/RustyClawClient
- **10 commits** on main (7 implementation + 1 fmt fix + 1 review fixes + 1 initial scaffold fix)
- **41 tests** (34 unit + 7 integration), clippy clean
- **Code review completed**, all 5 findings fixed

**Crate structure:**
```
crates/rustyclaw-client/src/
├── lib.rs       — Module declarations + pub use re-exports
├── error.rs     — WalletError, SignerError, ClientError (thiserror)
├── config.rs    — ClientConfig + ClientBuilder
├── wallet.rs    — Wallet wrapping solana_sdk::Keypair, BIP39, zeroize
├── signer.rs    — sign_exact_payment, build_payment_payload, encode_payment_header
└── client.rs    — RustyClawClient with chat(), models(), estimate_cost()
tests/
└── integration.rs — 7 wiremock-based end-to-end tests
```

**Key dependency workarounds discovered:**
- `solana-client` and `spl-associated-token-account` conflict with `solana-sdk 2.2` via transitive `zeroize` version incompatibilities
- **Solution:** Manual ATA derivation via `Pubkey::find_program_address`, reqwest JSON-RPC for blockhash fetching
- `zeroize` must NOT have `derive` feature (conflicts with `curve25519-dalek v3`)
- OpenSSL env vars required on this machine: `OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl`

### Current Workspace (RustyClawRouter — 5 crates)

```
crates/
├── protocol/    — rustyclaw-protocol: shared wire-format types (payment + chat + constants)
├── gateway/     — The only binary (Axum HTTP server) + ServiceRegistry
├── x402/        — Payment verification, Solana, escrow, fee payer (re-exports rustyclaw-protocol)
├── router/      — 15-dim scorer, routing profiles, model registry
└── cli/         — rcr CLI binary (clap derive)
```

### What's NOT Done Yet

**Remaining RustyClawRouter phases:**
| Phase | Description | Status |
|-------|-------------|--------|
| 8: Agent Delegation | Scoped session tokens + PDA sub-accounts | Not started |
| 9: x402 V2 Alignment | CAIP-2, wallet identity, Bazaar discovery | Not started |
| 12: Context Compression | Server-side prompt compression | Not started |
| 13: Staking-for-Capacity | DeFi primitive for inference allocation | Not started |
| 14: Cloudflare Workers Backend | x402 facilitator for CF Workers | Not started |

**RustyClawClient ecosystem** (master plan: `.claude/plan/rustyclaw-ecosystem.md`):
| Phase | Description | Status |
|-------|-------------|--------|
| A: Extract Protocol Crate | `rustyclaw-protocol` | ✅ Complete |
| B: Core Client Library | Wallet, signer, client | ✅ Complete |
| C: Proxy Sidecar | localhost OpenAI-compat proxy | Not started |
| D: CLI | `rustyclawclient` commands | Not started |
| E: Smart Features | Sessions, cache, degraded detection, free fallback | Not started |
| F: SDKs | Python, TS, Go client SDKs | Not started |
| G: Gateway Changes | Debug headers, session ID, stats endpoint | G.3 (heartbeat) ✅, rest not started |

---

## Next Steps (Pick One)

### Option 1: Phase C — Proxy Sidecar
A localhost proxy that speaks OpenAI format, so any existing OpenAI SDK client can use RustyClawRouter without code changes. Most impactful for adoption.

### Option 2: Phase D — CLI
`rustyclawclient` CLI binary for wallet management, chat, and cost estimation. Good for developer experience.

### Option 3: Gateway Phase 8 — Agent Delegation
Scoped session tokens with PDA sub-accounts. Now unblocked by RustyClawClient.

### Option 4: Phase E — Smart Features
Sessions, caching, degraded provider detection, free-tier fallback. Makes the client production-ready.

---

## What Worked (Cumulative)

- **Subagent-Driven Development** — fresh subagent per task with two-stage review. Kenneth's preferred execution approach.
- **Brainstorming skill before implementation** — one-question-at-a-time design. Kenneth prefers this.
- **Parallel subagent dispatch** — safe when tasks modify different files.
- **Research-backed design decisions** — Kenneth values this: "they are smart and if you look deep enough you will see the purpose."
- **Batching trivial sequential tasks** — Tasks 1-4 (all needed before first compile) dispatched as one batch. Saved context.
- **Doing trivial tasks directly** — single-line changes done by controller, not subagent.
- **Manual ATA derivation** — cleaner and lighter than pulling in `spl-associated-token-account` with its dep conflicts.
- **reqwest JSON-RPC for blockhash** — avoids `solana-client` entirely, no QUIC/rustls conflicts.

## What Didn't Work (Cumulative)

- **Concurrent agents modifying the same files** — parallel agents modified `lib.rs` and `integration.rs` simultaneously. Solution: never dispatch parallel agents that touch the same files.
- **`git add -A` in subagents** — swept up untracked docs/skills files. Solution: use specific `git add` paths.
- **Plan specified wrong migration number** — plan said `006_` but only `001_` existed. Agents adapted, but plans should verify existing state.
- **`solana-client` dep with `solana-sdk 2.2`** — transitive zeroize conflicts. Solution: use reqwest JSON-RPC directly.
- **`spl-associated-token-account` dep** — same conflict chain. Solution: manual ATA derivation via PDA.
- **`zeroize` derive feature** — conflicts with `curve25519-dalek v3`. Solution: omit `derive` feature.
- **Stale `GITHUB_TOKEN` in `.bashrc`** — overrode `gh auth login` credentials. Solution: removed env var.
- **`Keypair::to_bytes()` for zeroize** — returns a copy, not mutable internals. Documented as best-effort.
- **`expect()` in library code** — violated project conventions. Fixed by returning `Result` with `ClientError::Config`.

---

## Architecture Quick Reference

### RustyClawRouter Workspace (this repo)

```
crates/
├── protocol/    — rustyclaw-protocol (serde, serde_json, thiserror only)
├── gateway/     — Binary: rustyclawrouter. Axum server + ServiceRegistry
├── x402/        — PaymentVerifier trait, SolanaVerifier, Facilitator, escrow, fee payer
├── router/      — Scorer, profiles (eco/auto/premium/free), model registry
└── cli/         — rcr CLI binary
```

### Key Files
- `CLAUDE.md` — Project instructions, architecture rules, code conventions
- `.claude/plan/rustyclaw-ecosystem.md` — Master ecosystem plan (RustyClawClient phases A-G)
- `docs/plans/2026-03-10-extract-rustyclaw-protocol.md` — Protocol extraction plan (completed)
- `config/models.toml` — 27 models across 5 providers with pricing

### Test Commands
```bash
# RustyClawRouter
cargo test                            # 304 tests total
cargo test -p gateway                 # 199 tests (161 unit + 38 integration)
cargo test -p x402                    # 74 tests
cargo test -p rustyclaw-protocol      # 18 tests
cargo test -p router                  # 13 tests
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings

# RustyClawClient (~/projects/RustyClawClient/)
cd ~/projects/RustyClawClient
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test  # 41 tests
OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo clippy --all-targets --all-features -- -D warnings
```

---

## Decision Log (Cumulative)

| # | Decision | Rationale |
|---|----------|-----------|
| 1 | Per-model circuit breaker (hybrid) | Model-specific outages more common than full provider outages |
| 2 | Smart default fallback chains + agent override | Sensible defaults reduce friction, override gives power |
| 3 | Adaptive heartbeat (5s → 2s) | Balances overhead vs timeout prevention |
| 4 | Transparent fallback via X-RCR-Fallback header | Least complexity — works without client changes |
| 5 | Integrated middleware in gateway crate | Features naturally live where streaming/provider calls happen |
| 6 | Session sticking is client-side | Gateway stays stateless; session is per-agent context |
| 7 | Prefer escrow payment scheme | Safer for agent — only pays actual cost |
| 8 | Separate repos (client vs gateway) | Different deployment targets, release cycles, users |
| 9 | Shared protocol crate on crates.io | Single source of truth for wire format |
| 10 | HMAC-SHA256 session tokens (not JWT) | Simpler, no external deps |
| 11 | Payment + Chat types in one protocol crate | Client needs both; one crate guarantees wire compat |
| 12 | Protocol crate lives in RustyClawRouter repo | Atomic changes with gateway; publish when needed |
| 13 | Delete both x402-solana and rcr-common | Industry pattern: no "common" alongside protocol crate |
| 14 | Named `rustyclaw-protocol` | Communicates purpose (contract), already in ecosystem plan |
| 15 | Flat module structure with top-level re-exports | Simplest, easy to navigate, flat imports |
| 16 | No error types in protocol crate | Pure types crate; each consumer defines own errors |
| 17 | ServiceRegistry moved to gateway | Server-internal; every production project does this |
| 18 | Wrap solana_sdk::Keypair in Wallet | Gets Signer trait for free |
| 19 | Path dep for rustyclaw-protocol in client | Standard multi-repo dev pattern |
| 20 | solana-sdk 2.2 for client | Matches gateway, battle-tested |
| 21 | Manual ATA derivation (no spl-associated-token-account) | Avoids zeroize dep conflicts with solana-sdk 2.2 |
| 22 | reqwest JSON-RPC for blockhash (no solana-client) | Avoids QUIC/rustls/zeroize conflicts |
| 23 | Edition 2021 for client (not 2024) | solana-sdk compatibility |
| 24 | Only "exact" payment scheme (escrow filtered) | Escrow signing not yet implemented |
| 25 | prefer_escrow defaults to false | Until escrow signing is implemented |

## User Preferences

- Prefers **subagent-driven execution** (option 1) over parallel sessions
- Wants **deep competitive analysis** before building
- Building for **production** use on trading platform + telsi.ai
- Prefers **adaptive/smart defaults** with user overrides
- Prefers **transparent behavior** (least complexity for everyone)
- Rust edition 2021, MSRV 1.85, clippy pedantic
