# BlockRun ClawRouter — Competitive Analysis

**Date:** 2026-03-08
**Subject:** https://github.com/BlockRunAI/ClawRouter (v0.12.25, TypeScript)
**Purpose:** Identify improvements, failure points, and advantages for RustyClawRouter Phase 5

---

## 1. Architecture Comparison

| Dimension | BlockRun ClawRouter | RustyClawRouter |
|-----------|-------------------|-----------------|
| Language | TypeScript (Node.js) | Rust (Axum/Tokio) |
| Deployment model | Client-side CLI proxy (localhost:4040) | Server-side gateway (:8402) |
| Chain support | Dual-chain: Base (EVM) + Solana | Solana-first (EVM planned) |
| Payment SDK | @x402/evm + @x402/svm (Coinbase SDK) | Custom x402 crate |
| Model count | 41+ across 6 providers | 16 across 5 providers |
| Router | 14-dim scorer + LLM fallback classifier | 15-dim scorer (pure rule-based) |
| Caching | In-memory (200 entries, 10min TTL) | Redis-backed (optional, degrades gracefully) |
| Persistence | File-based (~/.openclaw/blockrun/) | PostgreSQL + Redis (both optional) |
| Session support | In-memory with auto-derived IDs | None yet |
| Compression | 7-layer context compression (15-40% savings) | None yet |
| Spend control | Time-windowed (per-request/hourly/daily/session) | wallet_budgets table (DB) |
| Escrow | None | Anchor PDA vault (deposit/claim/refund) |
| Key management | Client-side BIP-39 wallet gen + file storage | Client-side only — gateway never sees keys |

**Key takeaway:** BlockRun is a **local proxy** — it runs on the user's machine, holds their wallet key, and forwards requests to providers. RustyClawRouter is a **server-side gateway** — agents send pre-signed transactions and the gateway verifies + settles. These are fundamentally different trust models.

---

## 2. What BlockRun Does Well (Learn From)

### 2.1 LLM Fallback Classifier
- Rule-based scorer handles ~70-80% of requests; ambiguous cases (null tier) fall to a cheap LLM call (~$0.00003, 200-400ms)
- In-memory cache (1000 entries) amortizes LLM cost over repeated patterns
- **Our gap:** Our scorer returns a tier always — never admits uncertainty. Consider adding a confidence threshold that triggers fallback.

### 2.2 Session Persistence + Three-Strike Escalation
- Sessions track model selection across requests via `x-session-id` header or auto-derived hash
- If 3+ consecutive responses hash identically (repetition detection), the session auto-escalates to a higher tier
- **Our gap:** No session concept. For enterprise/dashboard features, sessions would let us track conversation costs and detect stuck loops.

### 2.3 Context Compression (7 Layers)
- Dedup → whitespace → dictionary encoding → path shortening → JSON compaction → observation compression → dynamic codebook
- Claims 15-40% token reduction, ~97% on tool results
- Codebook header injected into first user message for provider compatibility
- **Our gap:** Zero compression. For a server-side gateway handling many requests, this could materially reduce provider costs for our users.

### 2.4 Request Deduplication
- SHA-256 hash of canonicalized+timestamp-stripped request body
- In-flight requests share a single upstream call; completed responses cached 30s
- Prevents duplicate payments for retry storms
- **Our gap:** We have Redis replay protection (prevents double-spend), but no request dedup that coalesces identical in-flight requests.

### 2.5 Response Caching (LiteLLM-inspired)
- Canonicalized request hashing, 200-entry LRU with 10min TTL
- Skips caching for errors (400+) and oversized responses (>1MB)
- **Our status:** We have Redis caching but should add the canonicalization + error-skip logic.

### 2.6 Spend Controls (Time-Windowed)
- Per-request, hourly, daily, and session spending limits
- File-persisted at ~/.openclaw/blockrun/spending.json with 0o600 permissions
- Automatic 24h history pruning
- **Our gap:** We have `wallet_budgets` (per-wallet totals) but no time-windowed limits. Enterprise customers will want hourly/daily caps.

### 2.7 Comprehensive Model Registry
- 41+ models with capability flags: `toolCalling`, `vision`, `agentic`, `reasoning`
- Selector filters by capability before choosing within a tier
- Extensive alias system (e.g., "sonnet" -> claude-sonnet-4-20250514)
- **Our status:** We have aliases and 16 models. We should add capability flags to `config/models.toml` to enable intelligent filtering.

### 2.8 Doctor / Diagnostics CLI
- System info, wallet validation, network checks, usage stats, AI-powered analysis
- **Our status:** `rcr doctor` command exists but is basic. Their version is more comprehensive.

---

## 3. Where BlockRun Fails / Has Weaknesses

### 3.1 The 3500-Line Proxy Monolith
`src/proxy.ts` is ~3500 lines handling: SSE streaming, response dedup, caching, session management, compression, three-strike escalation, multi-chain payment, retries, and error handling — all in one file. This is:
- Untestable in isolation
- Impossible to reason about error propagation
- A merge conflict magnet
- **Our advantage:** Gateway crate separates concerns: `routes/`, `middleware/`, `providers/`, each <500 lines.

### 3.2 Client-Side Trust Model
BlockRun stores wallet private keys on disk (`~/.openclaw/blockrun/wallet.key`). The proxy holds the key and signs transactions. This means:
- Compromised machine = lost funds
- No server-side enforcement of spend limits (client can bypass)
- No multi-tenancy — one wallet per installation
- **Our advantage:** Gateway never sees private keys. Pre-signed transactions provide cryptographic proof without key custody.

### 3.3 In-Memory Everything
- Session store: in-memory (lost on restart)
- Response cache: in-memory (lost on restart)
- Dedup cache: in-memory (lost on restart)
- LLM classifier cache: in-memory (1000 entries, lost on restart)
- **Our advantage:** Redis + PostgreSQL persistence. Gateway restarts don't lose state.

### 3.4 No Escrow / No Trustless Settlement
BlockRun does direct payments only — transfer tokens then hope the provider responds. No escrow, no refund path, no dispute mechanism. If the provider returns garbage or times out after payment, funds are gone.
- **Our advantage:** Anchor escrow with deposit/claim/refund. Provider must deliver before claiming funds. Expiry-based automatic refund.

### 3.5 Single-User Architecture
No concept of tenants, teams, or API keys. One installation = one wallet = one user. Dashboard analytics would require a complete architectural rethink.
- **Our advantage:** Server-side architecture naturally supports multi-tenancy via wallet addresses as tenant identifiers.

### 3.6 No Rate Limiting
No rate limiter anywhere in the codebase. A misbehaving agent can drain the wallet with no throttle.
- **Our advantage:** Tower-based rate limiting middleware already implemented.

### 3.7 Retry Logic is Naive
- Exponential backoff for 429/502/503/504 only
- No circuit breaker — keeps hammering a failing provider
- No provider failover (if OpenAI is down, just fail)
- **Our opportunity:** Implement circuit breaker + provider failover chains.

### 3.8 Balance Monitoring is EVM-Only
`BalanceMonitor` checks USDC on Base only. Solana balance monitoring is absent. Users on Solana have no low-balance warnings.
- **Our advantage:** We have `balance_monitor.rs` for Solana with configurable thresholds.

### 3.9 No Replay Protection
No mechanism to prevent transaction replay. The dedup cache is 30s in-memory — restart the proxy and old transactions could potentially be replayed.
- **Our advantage:** Redis-backed replay protection with configurable TTL.

---

## 4. Features We Should Build (Phase 5 Priorities)

Based on this analysis, ranked by impact:

### HIGH — Build These
1. **Session tracking** — Track conversations across requests. Use wallet + session-id as key. Store in PostgreSQL. Enables: per-conversation cost attribution, stuck-loop detection, usage analytics.
2. **Time-windowed spend limits** — Add hourly/daily/monthly caps on top of wallet_budgets. Enterprise feature. Stored in DB, enforced server-side (unlike BlockRun's client-side limits).
3. **Model capability flags** — Add `tool_calling`, `vision`, `agentic`, `reasoning` booleans to `config/models.toml`. Use in smart router to filter models by capability before tier selection.
4. **Request deduplication** — Coalesce identical in-flight requests. Prevents duplicate provider calls during retry storms. Redis-backed (survives restarts).
5. **Dashboard backend APIs** — Real endpoints for: usage over time, cost by model/wallet, active sessions, spend limit management. Replace mock data.

### MEDIUM — Build After Core
6. **Confidence threshold on scorer** — When score is near tier boundary (e.g., within 0.05 of threshold), return "ambiguous" and allow fallback behavior (default to higher tier rather than LLM call — avoids adding latency).
7. **Provider failover chains** — If primary provider returns 5xx, try secondary. Define fallback chains per model family.
8. **Enhanced doctor command** — System diagnostics, Solana RPC health, provider reachability, wallet balance check, recent error summary.

### LOW — Nice to Have
9. **Context compression** — Server-side context compression before forwarding to providers. Complex to implement correctly without breaking semantics. Consider as opt-in feature.
10. **Response caching improvements** — Add canonicalization, skip error caching, LRU eviction. Most of the infrastructure exists in Redis already.
11. **Circuit breaker** — Track provider error rates. Open circuit after N failures in M seconds. Half-open test after cooldown.

---

## 5. What NOT to Copy

- **Client-side key storage** — Our server-side model is strictly better for security
- **In-memory-only persistence** — We already have Redis/PostgreSQL
- **LLM classifier fallback** — Adds 200-400ms latency and a payment dependency in the routing path. Our rule-based approach with a higher-tier default for ambiguous cases is faster and simpler
- **3500-line monolith pattern** — Keep our modular crate structure
- **BIP-39 wallet generation** — Not our concern; agents bring their own wallets
- **Dual-chain payment in Phase 5** — Stay Solana-first until Phase 6

---

## 6. Summary Matrix

| Category | BlockRun | RustyClawRouter | Winner |
|----------|----------|-----------------|--------|
| Latency | Node.js + localhost | Rust + server-side | RCR (Rust perf) |
| Security | Client-side keys | Pre-signed txs, no key custody | RCR |
| Escrow | None | Anchor PDA vault | RCR |
| Replay protection | None (30s in-memory dedup) | Redis-backed | RCR |
| Rate limiting | None | Tower middleware | RCR |
| Model count | 41+ | 16 | BlockRun |
| Session tracking | Yes (in-memory) | Not yet | BlockRun |
| Compression | 7-layer, 15-40% savings | None | BlockRun |
| Spend controls | Time-windowed (client-side) | Wallet budgets (server-side) | Tie (different strengths) |
| Multi-tenancy | Single-user only | Wallet-based tenancy | RCR |
| Persistence | File-based | PostgreSQL + Redis | RCR |
| Code quality | 3500-line monolith | Modular crates (<500 LOC each) | RCR |
| Diagnostics | Comprehensive doctor | Basic doctor | BlockRun |

**Bottom line:** RustyClawRouter has a fundamentally stronger architecture (server-side, Rust, persistent state, escrow, modular). BlockRun has more features around UX (sessions, compression, model breadth, diagnostics). Phase 5 should close the feature gaps that matter (sessions, time-windowed limits, capability flags, dedup) while maintaining our architectural advantages.
