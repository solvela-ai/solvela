# HANDOFF.md — RustyClawRouter Continuation Guide

> **Start here.** This document captures the full context from the previous session so a fresh agent can continue without ramp-up time.

---

## Goal

Build the **RustyClaw ecosystem** — a self-sovereign, Solana-native AI agent payment stack consisting of:

1. **RustyClawRouter** (gateway server) — verifies x402 payments, routes to LLM providers, settles on Solana. **This repo.**
2. **RustyClawClient** (client library) — holds wallet, signs payments, makes LLM calls transparent. **Not yet built.**
3. **rustyclaw-protocol** (shared wire format) — x402 types used by both. **Partially exists as `x402-solana` crate.**

The user (Kenneth) is building this for his **trading platform** (currently named /rustyclaw, to be renamed) and **AI assistant platform** (telsi.ai).

---

## Current Progress

### What's Complete in RustyClawRouter

**Phases 1-7 of the ecosystem upgrade plan** (`docs/plans/2026-03-09-ecosystem-upgrade-plan.md`):
- Phase 1: Wire format foundation (Developer role, tool calls, vision types, streaming deltas)
- Phase 2: Durable escrow claim queue (PostgreSQL-backed, 5-retry limit)
- Phase 3: Expanded model registry (16 → 27 models, 5 providers)
- Phase 4: Session tokens (HMAC-SHA256, `SessionClaims` with wallet/budget/models/expiry)
- Phase 5: Extracted `x402-solana` crate (shared types, zero framework deps)
- Phase 6: ElizaOS plugin (`integrations/elizaos/`)
- Phase 7: Dashboard API stub (`GET /v1/dashboard/spend`)

**Phases 10-11 (SSE Heartbeat + Provider Failover)** — just completed this session:
- Adaptive SSE heartbeat stream wrapper (5s → 2s after 10s silence)
- Per-model circuit breaker (model-specific outages don't block other models)
- Model-level fallback chains (Opus → GPT-5.2 → Gemini Pro, etc.)
- `X-RCR-Fallback` header for transparent fallback reporting
- `X-RCR-Fallback-Preference` header for agent-specified override
- 305 total workspace tests passing, clippy clean

### What's NOT Done Yet

**Remaining RustyClawRouter phases:**
| Phase | Description | Status |
|-------|-------------|--------|
| 8: Agent Delegation | Scoped session tokens + PDA sub-accounts | Brainstorming started, blocked on RustyClawClient |
| 9: x402 V2 Alignment | CAIP-2, wallet identity, Bazaar discovery | Not started |
| 12: Context Compression | Server-side prompt compression | Not started |
| 13: Staking-for-Capacity | DeFi primitive for inference allocation | Not started |
| 14: Cloudflare Workers Backend | x402 facilitator for CF Workers | Not started |

**RustyClawClient ecosystem** (the master plan is in `.claude/plan/rustyclaw-ecosystem.md`):
| Phase | Description | Status |
|-------|-------------|--------|
| A: Extract Protocol Crate | `rustyclaw-protocol` on crates.io | Partially done (`x402-solana` exists) |
| B: Core Client Library | Wallet, signer, client (the main deliverable) | Not started |
| C: Proxy Sidecar | localhost OpenAI-compat proxy | Not started |
| D: CLI | `rustyclawclient` commands | Not started |
| E: Smart Features | Sessions, cache, degraded detection, free fallback | Not started |
| F: SDKs | Python, TS, Go client SDKs | Not started |
| G: Gateway Changes | Debug headers, session ID, stats endpoint | G.3 (heartbeat) ✅, rest not started |

---

## What Worked

- **Subagent-Driven Development** — dispatching fresh subagent per task with two-stage review (spec compliance then code quality) produced clean, tested code efficiently. Used for both the ecosystem upgrade (13 tasks) and the heartbeat/failover feature (9 tasks).
- **Brainstorming skill before implementation** — structured one-question-at-a-time design prevented premature implementation. The user prefers this workflow.
- **Parallel subagent dispatch** — independent tasks (e.g., Task 3 + Task 4) dispatched in parallel safely since they modify different files.
- **Per-model circuit breaker over per-provider** — research showed model-specific outages are significantly more common than full provider outages (Anthropic, OpenAI status page data confirms this).
- **Hybrid approach with provider escalation** — model-level failures cascade to provider-level tracking, catching both partial and full outages.

## What Didn't Work

- **Concurrent agents modifying the same files** — in the first plan execution, parallel agents modified `lib.rs` and `integration.rs` simultaneously, causing interference. Solution: never dispatch parallel agents that touch the same files.
- **`x402-solana` type identity issue** — when extracting `CostBreakdown` to a separate crate, there was a "mismatched types" risk. Solution: make `rcr-common` depend on `x402-solana` and re-export, ensuring single type identity.
- **Plan specified wrong migration number** — plan said `006_` but only `001_` existed. Agents correctly adapted to use `002_`.

---

## Architecture Quick Reference

### Workspace Crates (`crates/`)
- **gateway** — The only binary. Axum HTTP server. Binary: `rustyclawrouter`.
- **x402** — Pure protocol library. Payment types, Solana verification, escrow, fee payer pool, nonce pool.
- **router** — 15-dimension rule-based scorer, routing profiles (eco/auto/premium/free), model registry.
- **common** (`rcr-common`) — Shared types: ChatRequest, ChatResponse, ModelInfo, CostBreakdown.
- **cli** (`rcr-cli`) — Gateway-side CLI (clap derive).

### Standalone
- **programs/escrow/** — Anchor escrow program (NOT workspace member due to dep conflicts).
- **x402-solana** — Extracted wire format types (the proto-protocol crate).

### Key Files
- `CLAUDE.md` — Project instructions, architecture rules, code conventions
- `.claude/plan/rustyclaw-ecosystem.md` — Master ecosystem plan (RustyClawClient phases A-G)
- `docs/plans/2026-03-09-ecosystem-upgrade-plan.md` — Detailed plan for phases 1-7 + future stubs
- `docs/plans/2026-03-09-sse-heartbeat-provider-failover.md` — Detailed plan for phases 10-11
- `config/models.toml` — 27 models across 5 providers with pricing

### Test Commands
```bash
cargo test                        # 305 tests total
cargo test -p gateway             # 191 tests (153 unit + 38 integration)
cargo test -p x402                # 74 tests
cargo test -p router              # 13 tests
cargo test -p rcr-common          # 20 tests
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
```

---

## Next Steps

The user was deciding what to build next when we ran out of context. The conversation ended at:

> **User asked about Phase 8 (Agent Delegation) → realized RustyClawClient needs to exist first → reviewed the ecosystem plan → was about to choose between:**
> - **A) Follow the plan — Phase A then B** (extract protocol crate, then build RustyClawClient core)
> - **B) Jump to Phase B** (use `x402-solana` as-is, rename to `rustyclaw-protocol` later)
> - **C) Something else**

**Recommended approach:**
1. Ask the user which option they chose (A, B, or C)
2. If A: use brainstorming skill to design the protocol crate extraction, then writing-plans skill for implementation
3. If B: use brainstorming skill to design the RustyClawClient core library (wallet → signer → client), noting it will be a **new repo**
4. Use subagent-driven-development skill for execution (user has chosen this approach both times)

**Important user preferences:**
- Prefers subagent-driven execution (option 1) over parallel sessions
- Wants deep competitive analysis before building ("they are smart and if you look deep enough you will see the purpose")
- Building for production use on his own trading platform + telsi.ai
- Prefers adaptive/smart defaults with user overrides (chose option D consistently)
- Prefers transparent behavior (X-RCR-Fallback header approach — least complexity for everyone)

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
| 10 | HMAC-SHA256 session tokens (not JWT) | Simpler, no external deps, eliminates 402 double round-trip |
