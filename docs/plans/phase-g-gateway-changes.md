# Phase G: Gateway Changes — Implementation Plan

> **Status:** Planned
> **Author:** Kenneth + Claude brainstorming session (2026-03-11)
> **Build Order:** G.2 (Debug Headers) → G.5 (Stats Endpoint) → G.1 (Session ID)
> **Prerequisites:** G.3 (SSE Heartbeat) ✅ and G.4 (Nonce Endpoint) ✅ already complete

---

## Overview

Three targeted gateway changes to support the Solvela Client ecosystem (Rust client, Python/TS/Go SDKs, `rcr`/`rcc` CLIs). No architectural changes — additive, backward-compatible modifications.

| Item | Purpose | Audience |
|------|---------|----------|
| G.2 Debug Headers | Routing diagnostics in response headers | Developers debugging routing/payment issues |
| G.5 Stats Endpoint | Per-wallet spend history | CLI `stats` command, SDK `stats()` method, future dashboard |
| G.1 Session ID | Conversation-level request grouping | Client SDKs for session cost tracking |

---

## G.2: Debug Response Headers

### Design

**Approach: Hybrid** — Request ID in middleware (available everywhere), debug headers assembled inline in the chat handler.

### Request ID Middleware

New middleware layer, outermost in the stack (before tracing).

**File:** `crates/gateway/src/middleware/request_id.rs`

**Behavior:**
1. Check for incoming `X-Request-Id` header
2. If present and valid → use it
3. If missing or invalid → generate `Uuid::new_v4`
4. Store in request extensions as `RequestId(String)`
5. **Always** attach `X-RCR-Request-Id` to response (not gated by debug flag)
6. Include in tracing span for log correlation

**Validation rules for client-provided IDs:**
- Max 128 characters
- Only `[a-zA-Z0-9\-_]`
- If invalid, silently replace with server-generated UUID (don't error on a debug feature)

**Layer position in `build_router()`:** Outermost, before `TraceLayer`, so the request ID appears in all tracing spans.

### Debug Headers (Conditional)

Only returned when request includes `X-RCR-Debug: true`.

**File:** `crates/gateway/src/routes/debug_headers.rs`

**Helper struct and function:**
```rust
struct DebugInfo {
    model: String,
    tier: String,
    score: f64,
    profile: String,
    provider: String,
    cache_status: String,       // "hit" | "miss" | "skip"
    latency_ms: u64,
    payment_status: String,     // "verified" | "cached" | "free" | "none"
    token_estimate_in: u32,
    token_estimate_out: u32,
}

fn attach_debug_headers(response: &mut Response, info: &DebugInfo)
```

> **Verify:** The scorer returns `ClassifyResult` with a tier classification. Confirm which field maps to the `score: f64` value. If the scorer only produces tier enums, replace `score` with the tier name or include raw dimension scores.

**Headers attached:**
```
X-RCR-Model: anthropic/claude-sonnet-4.6
X-RCR-Tier: Complex
X-RCR-Score: 0.4237
X-RCR-Profile: auto
X-RCR-Provider: anthropic
X-RCR-Cache: miss
X-RCR-Latency-Ms: 1847
X-RCR-Payment-Status: verified
X-RCR-Token-Estimate-In: 1200
X-RCR-Token-Estimate-Out: 500
```

**Chat handler flow:**
1. Record `Instant::now()` at handler start
2. After routing: capture model, tier, score, profile, provider
3. After cache check: capture cache status
4. After payment: capture payment status
5. After provider response: capture token counts
6. Before return: if `X-RCR-Debug: true`, call `attach_debug_headers()`
7. Latency = `start.elapsed().as_millis()`

**Files modified:**
- New: `crates/gateway/src/middleware/request_id.rs`
- New: `crates/gateway/src/routes/debug_headers.rs`
- Modify: `crates/gateway/src/middleware/mod.rs` — export request_id module
- Modify: `crates/gateway/src/routes/mod.rs` — export debug_headers module
- Modify: `crates/gateway/src/routes/chat.rs` — check debug flag, assemble DebugInfo, call attach
- Modify: `crates/gateway/src/lib.rs` — add RequestId layer to build_router()

### Tests (15)

1. Request without debug flag → response has `X-RCR-Request-Id` only, no other `X-RCR-*` debug headers
2. Request with `X-RCR-Debug: true` → all 10 debug headers present
3. Request with `X-RCR-Debug: false` → no debug headers (only request ID)
4. Client-provided `X-Request-Id` echoed back
5. Invalid client `X-Request-Id` (too long) → replaced with server UUID
6. Invalid client `X-Request-Id` (special chars) → replaced with server UUID
7. Request ID present on error responses (402)
8. Request ID present on 500 error responses
9. Request ID present on streaming responses
10. Debug headers present on streaming responses when flag set
11. Cache hit reflected in `X-RCR-Cache: hit`
12. Payment status `verified` when payment provided
13. Payment status `free` for free tier model
14. Payment status `none` when no payment (402 response)
15. Debug headers not leaked when flag is absent (security)

---

## G.5: Stats Endpoint

### Design

**Route:** `GET /v1/wallet/{address}/stats?days=30`

**Auth:** `Authorization: Bearer <session-token>` — reuses existing `x-rcr-session` HMAC token. Handler verifies token and checks wallet address inside it matches `{address}` path param. Mismatch → 403.

> **⚠️ Known issue:** The current `build_session_token()` in `chat.rs` populates the `wallet` field from `payload.accepted.pay_to` (the gateway's recipient wallet), not the payer's wallet. This means token-to-path-param matching will always fail. **Before implementing stats auth, fix `extract_payment_info()` to return the actual payer wallet address** (from the transaction signer), or use a different auth mechanism.

**Query params:**
- `days` — integer, default 30, max 365, min 1. Values outside range → 400.

### Response Shape

```json
{
  "wallet": "7xKX...abc",
  "period_days": 30,
  "summary": {
    "total_requests": 1247,
    "total_cost_usdc": "3.847291",
    "total_input_tokens": 892400,
    "total_output_tokens": 341200
  },
  "by_model": [
    {
      "model": "anthropic/claude-sonnet-4.6",
      "requests": 412,
      "cost_usdc": "1.923000",
      "input_tokens": 310000,
      "output_tokens": 142000
    }
  ],
  "by_day": [
    {
      "date": "2026-03-11",
      "requests": 47,
      "cost_usdc": "0.142300"
    }
  ]
}
```

### Database Queries

Three queries, can run concurrently with `tokio::join!`:

```sql
-- Summary
SELECT COUNT(*) as total_requests,
       COALESCE(SUM(cost_usdc), 0) as total_cost,
       COALESCE(SUM(input_tokens), 0) as total_input,
       COALESCE(SUM(output_tokens), 0) as total_output
FROM spend_logs
WHERE wallet_address = $1
  AND created_at >= NOW() - make_interval(days => $2);

-- By model
SELECT model, COUNT(*) as requests,
       SUM(cost_usdc) as cost,
       SUM(input_tokens) as input_tokens,
       SUM(output_tokens) as output_tokens
FROM spend_logs
WHERE wallet_address = $1
  AND created_at >= NOW() - make_interval(days => $2)
GROUP BY model ORDER BY cost DESC;

-- By day
SELECT DATE(created_at) as date,
       COUNT(*) as requests,
       SUM(cost_usdc) as cost
FROM spend_logs
WHERE wallet_address = $1
  AND created_at >= NOW() - make_interval(days => $2)
GROUP BY DATE(created_at) ORDER BY date;
```

### Error Handling

- No PostgreSQL configured → `503 Service Unavailable` with `{"error": "stats unavailable — no database configured"}`
- Valid wallet, no data → 200 with zeros and empty arrays (never 404)
- Invalid `days` param → 400
- Missing auth → 401
- Invalid token → 401
- Token wallet ≠ path wallet → 403

### Files

- Rename: `crates/gateway/src/routes/dashboard.rs` → `crates/gateway/src/routes/stats.rs`
- Modify: `crates/gateway/src/routes/mod.rs` — update module name
- Modify: `crates/gateway/src/lib.rs` — update route from `/v1/dashboard/spend` to `/v1/wallet/:address/stats`
- Modify: `crates/gateway/src/usage.rs` — add `get_wallet_stats()`, `get_stats_by_model()`, `get_stats_by_day()` query methods

### Tests (10)

1. Valid session token + matching wallet → 200 with correct stats shape
2. Valid token + mismatched wallet → 403
3. Missing auth header → 401
4. Invalid token → 401
5. Default days (omitted param) → 30
6. Explicit days=7 → returns 7 days of data
7. Days > 365 → 400
8. Days < 1 → 400
9. Empty results → 200 with zeros and empty arrays
10. No database → 503

---

## G.1: Session ID Support

### Design

**Incoming:** Read `X-Session-Id` header in the chat handler. Validate: ≤ 128 chars, `[a-zA-Z0-9\-_]` only. Invalid → ignore (treat as absent).

**Outgoing:** Echo `X-Session-Id` back in the response if one was provided. Separate from existing `x-rcr-session` (HMAC auth token).

**Logging:** Pass session ID to the `spend_logs` DB write. If not provided → `null`.

**No server-generated session IDs.** If the client doesn't send one, it's simply absent. Request ID handles per-request tracking.

### Files

- Modify: `crates/gateway/src/routes/chat.rs` — extract `X-Session-Id`, echo in response, pass to usage logging
- Modify: `crates/gateway/src/usage.rs` — accept `request_id` and `session_id` in log function, include in INSERT

> **Refactor:** Instead of adding 2 more positional args to `log_spend()` (already at 7, would become 9), refactor to accept a `SpendLogEntry` struct. This keeps the API clean and follows project conventions.

### Tests (6)

1. `X-Session-Id` sent → echoed in response
2. No `X-Session-Id` → header not present in response
3. Session ID persisted to spend_logs
4. No session ID → null in spend_logs
5. Oversized session ID (>128 chars) → ignored, not echoed
6. Request ID also persisted to spend_logs

---

## Database Migration

**Single migration for all Phase G changes.**

**File:** `migrations/003_phase_g_request_session_ids.sql`

```sql
-- Phase G: Add request_id and session_id tracking to spend_logs
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS request_id TEXT DEFAULT NULL;
ALTER TABLE spend_logs ADD COLUMN IF NOT EXISTS session_id TEXT DEFAULT NULL;
CREATE INDEX IF NOT EXISTS idx_spend_session ON spend_logs(session_id) WHERE session_id IS NOT NULL;
```

### Migration Tests (2)

1. New columns exist after migration
2. Partial index on session_id created

---

## Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| 66 | Debug headers opt-in via `X-RCR-Debug: true` | Always-on, env-based, allowlist-gated | No info leakage by default; matches Cloudflare `cf-debug` pattern |

> **Defense in depth:** Consider adding a server-side `RCR_DEBUG_ENABLED` env var (default: `true` in dev, `false` in production). When disabled, ignore `X-RCR-Debug: true` from clients. Low-cost safeguard for production deployments handling financial transactions.
| 67 | 11 debug headers (7 original + request ID + payment status + token estimates) | Original 7 only | All data already computed; headers are negligible overhead |
| 68 | Request ID: client-provided with server fallback | Server-only, client-required | Industry standard for distributed tracing; enables end-to-end correlation |
| 69 | Request ID always returned (not gated by debug flag) | Debug-only | Zero security risk (random UUID); massive operational value; matches Stripe/AWS/GitHub |
| 70 | Hybrid: Request ID in middleware, debug headers in handler | All-middleware, all-inline | Request ID needs early availability; debug data is local to handler |
| 71 | Stats path: `GET /v1/wallet/{address}/stats` | `/v1/stats?wallet=`, `/v1/dashboard/spend` | RESTful resource-oriented; self-documenting |
| 72 | Stats time range: `?days=N` (default 30, max 365) | Date ranges, preset periods | YAGNI; covers 90% of cases |
| 73 | Stats shape: summary + by_model + by_day | Summary only, add by_provider | Covers CLI, SDK, and dashboard needs |
| 74 | Stats auth: reuse `x-rcr-session` HMAC token | Signed challenge, API keys, no auth | Zero new infrastructure; token already issued and verified |
| 75 | Session ID: echo + log, no server-side sticking | Echo-only, server sticking + escalation | Client handles sticking; server adds cost tracking without duplicating logic |
| 76 | No server-generated session IDs | Auto-generate per request, per time window | Sessions are a client concept; Request ID covers per-request tracking |
| 77 | Single migration for request_id + session_id | Separate migrations per feature | Both are simple ALTER TABLE; one migration is cleaner |
| 78 | Partial index on session_id (WHERE NOT NULL) | Full index, no index | Most rows null initially; partial index avoids bloat |
| 79 | Build order: G.2 → G.5 → G.1 | Any other order | Debug headers are foundation; stats is more complex; session ID is simplest |

---

## Non-Functional Requirements

- **Performance:** Debug headers add < 1us (string formatting). Request ID middleware adds UUID generation (~50ns). Stats queries use existing indexes on `wallet_address` and `created_at`.
- **Security:** No routing internals leaked without explicit `X-RCR-Debug: true`. Stats require HMAC token proving wallet ownership. Session IDs validated for size/charset.
- **Reliability:** All features degrade gracefully — no DB means stats returns 503, no Redis has no impact on any G items.
- **Backward compatibility:** All changes are additive. No existing headers or endpoints removed. Old route `/v1/dashboard/spend` removed (was a stub returning zeros — no real consumers).

---

## Implementation Checklist

### G.2: Debug Headers
- [ ] Create `crates/gateway/src/middleware/request_id.rs`
- [ ] Create `crates/gateway/src/routes/debug_headers.rs`
- [ ] Add RequestId layer to `build_router()` in `lib.rs`
- [ ] **CORS update required in `build_cors()` (`lib.rs`):**
  - Add `.expose_headers(["X-RCR-Request-Id", "X-RCR-Debug", "X-RCR-Provider", "X-RCR-Model", "X-RCR-Tier", "X-RCR-Score", "X-RCR-Route-Time-Ms", "X-RCR-Cache-Status", "X-RCR-Payment-Status", "X-RCR-Prompt-Tokens-Est", "X-RCR-Completion-Tokens-Est", "X-Session-Id"])` so browser clients can read custom response headers.
  - Add `X-RCR-Debug`, `X-Request-Id`, and `X-Session-Id` to `.allow_headers([...])` so browser clients can send custom request headers.
- [ ] Add RequestId to tracing spans
- [ ] Check `X-RCR-Debug: true` in chat handler
- [ ] Assemble `DebugInfo` from routing/payment/cache data in chat handler
- [ ] Call `attach_debug_headers()` before response return
- [ ] Attach `X-RCR-Request-Id` on all responses (including errors, streaming)
- [ ] Write 15 tests
- [ ] Verify: `cargo test -p gateway`
- [ ] Verify: `cargo clippy --all-targets --all-features -- -D warnings`

### G.5: Stats Endpoint
- [ ] Rename `routes/dashboard.rs` → `routes/stats.rs`
- [ ] Update route in `lib.rs`: `/v1/wallet/:address/stats`
- [ ] Implement session token auth check with wallet match
- [ ] Add `get_wallet_stats()` to `usage.rs`
- [ ] Add `get_stats_by_model()` to `usage.rs`
- [ ] Add `get_stats_by_day()` to `usage.rs`
- [ ] Wire queries into handler with `tokio::join!`
- [ ] Handle no-DB (503), empty results (200 + zeros), invalid params (400)
- [ ] Write 10 tests
- [ ] Verify: `cargo test -p gateway`
- [ ] Verify: `cargo clippy --all-targets --all-features -- -D warnings`

### G.1: Session ID
- [ ] Extract `X-Session-Id` in chat handler
- [ ] Validate size/charset
- [ ] Echo in response header
- [ ] Pass to usage log function
- [ ] Update `usage.rs` INSERT to include `request_id` and `session_id`
- [ ] Write 6 tests
- [ ] Verify: `cargo test -p gateway`
- [ ] Verify: `cargo clippy --all-targets --all-features -- -D warnings`

### Migration
- [ ] Create `migrations/003_phase_g_request_session_ids.sql`
- [ ] Write 2 migration tests
- [ ] Verify migration runs idempotently

### Final
- [ ] Full test suite passes: `cargo test` (all workspace)
- [ ] `cargo fmt --all -- --check` clean
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` clean
- [ ] Update HANDOFF.md with Phase G completion
