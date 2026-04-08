# Phase 8: Escrow Hardening

> Operational hardening of the escrow subsystem — claim processing, fee payer rotation, durable nonces, error recovery, monitoring, and a config endpoint.

**Status:** Complete
**Depends on:** Phases 1-7, Phase G (all complete, 342 tests passing)
**Estimated scope:** ~15 new tests, ~600 lines of new/modified Rust

---

## Overview

The escrow subsystem works end-to-end but has several production gaps:

1. **Claim processor auto-start** is already wired in `main.rs` but lacks graceful shutdown.
2. **Fee payer rotation** — `FeePayerPool` has round-robin + failover logic but the `EscrowClaimer` only uses a single key (the one passed at construction). Claims should use the pool.
3. **Durable nonces** — claim transactions use `getLatestBlockhash` (60s expiry). Under load or RPC latency, claims can expire before landing. The `NoncePool` exists but is only exposed to clients via `GET /v1/nonce`, not used internally for claims.
4. **Error recovery** — `claim_processor.rs` retries with a flat polling interval and `MAX_CLAIM_ATTEMPTS=5`. No exponential backoff, no circuit breaker.
5. **No escrow config endpoint** — clients cannot discover escrow parameters without making a payment attempt.
6. **No escrow-specific monitoring** — fee payer SOL balances are monitored via `BalanceMonitor` for the recipient wallet, but not for fee payer pool wallets. No claim success/failure counters.

---

## 8.1: Claim Processor Auto-Start (with Graceful Shutdown)

The claim processor is already spawned in `main.rs` (lines 293-300). This item adds graceful shutdown via `tokio::sync::watch` or `tokio_util::sync::CancellationToken`.

### Files to Modify

- `crates/x402/src/escrow/claim_processor.rs` — accept a shutdown signal
- `crates/gateway/src/main.rs` — wire shutdown signal to SIGTERM

### Design

- Add a `shutdown: tokio::sync::watch::Receiver<bool>` parameter to `start_claim_processor`.
- In the poll loop, `tokio::select!` between `interval.tick()` and `shutdown.changed()`.
- On shutdown, drain in-progress claims (finish current batch, don't pick up new ones).
- In `main.rs`, create the watch channel. Use `axum::serve(...).with_graceful_shutdown(signal)` and signal the claim processor on the same trigger.

### Implementation Steps

1. Add `shutdown_rx: tokio::sync::watch::Receiver<bool>` parameter to `start_claim_processor`.
2. Replace the bare `loop` with `tokio::select!` that exits on shutdown signal.
3. In `main.rs`, create `tokio::sync::watch::channel(false)` and pass `rx` to the processor.
4. Wire `tokio::signal::ctrl_c()` (and `SIGTERM` on Unix) to send `true` on the `tx`.
5. Pass the same future to `axum::serve(...).with_graceful_shutdown(...)`.

---

## 8.2: Fee Payer Rotation for Claims

Currently `EscrowClaimer` stores a single `fee_payer_keypair: [u8; 64]` and uses it for all claims. The `FeePayerPool` (with round-robin + cooldown) exists but is not used by the claimer.

### Files to Modify

- `crates/x402/src/escrow/claimer.rs` — accept `Arc<FeePayerPool>` instead of a single key
- `crates/x402/src/escrow/claim_processor.rs` — pass pool-selected wallet to each claim
- `crates/gateway/src/main.rs` — construct `EscrowClaimer` with the pool

### Design

- Replace `fee_payer_keypair: [u8; 64]` in `EscrowClaimer` with `fee_payer_pool: Arc<FeePayerPool>`.
- In `do_claim`, call `pool.next()` to get the current wallet. If the claim fails with an RPC error suggesting insufficient SOL, call `pool.mark_failed(wallet.index)`.
- The existing `FeePayerPool` already handles:
  - Round-robin via `AtomicUsize` counter
  - Cooldown-based recovery (default 60s)
  - `AllFailed` error when all wallets are in cooldown
- `EscrowClaimer::new` signature changes: accept `Arc<FeePayerPool>` instead of `fee_payer_b58_key`.
- Remove the constraint that `fee_payer_pubkey == recipient_wallet` (the pool may contain wallets that are not the recipient). The claim instruction's `provider` account should be the `recipient_wallet`, and the `fee_payer` is a separate signer. This requires the two-signer transaction path — which the current code explicitly rejects. **Decision needed: keep single-signer constraint or implement two-signer claims.**

### Implementation Steps

1. Change `EscrowClaimer` to hold `Arc<FeePayerPool>` + `recipient_wallet` + `usdc_mint` (drop `fee_payer_keypair`).
2. In `do_claim`, call `self.fee_payer_pool.next()` to select wallet.
3. If claim submission returns an RPC error containing "insufficient lamports" or similar, call `pool.mark_failed(wallet.index)` and return error (the claim processor will retry on next cycle with a different wallet).
4. Update `EscrowClaimer::new` constructor — accept pool instead of single key.
5. Update `main.rs` — pass the already-constructed `fee_payer_pool` to `EscrowClaimer`.
6. Update `Drop` impl — pool handles its own zeroing.
7. Keep the single-signer constraint for now (Decision #80) — the fee payer key must equal the recipient wallet key. Document this limitation.

---

## 8.3: Durable Nonces for Claims

Claim transactions currently call `getLatestBlockhash` which expires after ~60 seconds. Under high load or RPC congestion, this window is too tight.

### Files to Modify

- `crates/x402/src/escrow/claimer.rs` — use nonce instead of blockhash when available
- `crates/x402/src/escrow/claim_processor.rs` — pass `NoncePool` reference

### Design

- In `do_claim`, check if a `NoncePool` is provided and non-empty.
- If available: fetch the nonce value via `NoncePool::fetch_nonce_value()`, use it as the blockhash in the transaction, and prepend an `AdvanceNonce` instruction (SystemProgram instruction index 4) as the first instruction.
- If unavailable or fetch fails: fall back to `getLatestBlockhash` with a `tracing::warn!`.
- The `AdvanceNonce` instruction layout:
  - Program: SystemProgram (11111111111111111111111111111111)
  - Accounts: nonce_account (writable), sysvar_recent_blockhashes, nonce_authority (signer)
  - Data: `[4, 0, 0, 0]` (AdvanceNonceAccount instruction index as u32 LE)

### Implementation Steps

1. Add `nonce_pool: Option<Arc<NoncePool>>` to `EscrowClaimer`.
2. In `do_claim`, attempt to fetch a nonce entry and its on-chain value.
3. If successful, build the transaction with the nonce value as blockhash and prepend `AdvanceNonce` instruction.
4. If fetch fails, fall back to regular blockhash with a warning log.
5. Update `main.rs` to pass the nonce pool to the claimer.

---

## 8.4: Error Recovery for Claims

Currently `claim_processor.rs` retries at a fixed 10-second interval with `MAX_CLAIM_ATTEMPTS=5`. This needs exponential backoff and a circuit breaker.

### Files to Modify

- `crates/x402/src/escrow/claim_processor.rs` — backoff + circuit breaker
- `crates/x402/src/escrow/claim_queue.rs` — increase `MAX_CLAIM_ATTEMPTS` to 10, add `next_retry_at` column

### Design

**Exponential backoff:**
- Store `next_retry_at` timestamp per claim in the DB.
- Backoff schedule: 1s, 2s, 4s, 8s, 16s, 32s, 64s, 128s, 256s, 300s (capped at 5min).
- `fetch_pending_claims` query adds `AND (next_retry_at IS NULL OR next_retry_at <= NOW())`.
- `mark_attempt_failed` sets `next_retry_at = NOW() + interval`.

**Circuit breaker:**
- Track rolling failure count in-memory: `(successes, failures)` in a 5-minute window.
- If `failures / (successes + failures) > 0.5` and `total >= 4` (minimum sample), pause processing for 60 seconds.
- Log a structured `tracing::error!` when the circuit opens, and `tracing::info!` when it closes.
- Use a simple `struct ClaimCircuitBreaker` with `Instant`-based windowing.

**Max retries:**
- Increase `MAX_CLAIM_ATTEMPTS` from 5 to 10.
- After 10 failures, mark as `failed` with final error message.

### Implementation Steps

1. Add migration: `ALTER TABLE escrow_claim_queue ADD COLUMN next_retry_at TIMESTAMPTZ`.
2. Update `fetch_pending_claims` to filter by `next_retry_at`.
3. Update `mark_attempt_failed` to compute and set `next_retry_at` based on attempt number.
4. Increase `MAX_CLAIM_ATTEMPTS` to 10.
5. Add `ClaimCircuitBreaker` struct to `claim_processor.rs` with `record_success()`, `record_failure()`, `is_open()`.
6. Check circuit breaker at start of each processing cycle; skip if open.
7. Add structured tracing for all state transitions.

---

## 8.5: GET /v1/escrow/config Endpoint

Public endpoint that returns escrow configuration for clients to discover escrow parameters without making a payment attempt.

### Files to Modify

- `crates/gateway/src/routes/mod.rs` — add `escrow` module
- `crates/gateway/src/routes/escrow.rs` — new file
- `crates/gateway/src/lib.rs` — register route

### Design

**Request:** `GET /v1/escrow/config` — no auth required.

**Response (200):**
```json
{
  "escrow_program_id": "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
  "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
  "usdc_mint": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
  "provider_wallet": "RecipientWallet...",
  "current_slot": 298765432
}
```

**Response (404):** `{ "error": "escrow not configured" }` when `escrow_program_id` is `None`.

**Current slot:** Fetched from Solana RPC via `getSlot`. Cached for 5 seconds using `tokio::sync::Mutex<(u64, Instant)>` on `AppState` (or a dedicated `SlotCache`).

### Implementation Steps

1. Create `crates/gateway/src/routes/escrow.rs` with `escrow_config` handler.
2. Add `SlotCache` to `AppState` — `Arc<tokio::sync::Mutex<Option<(u64, Instant)>>>`.
3. Handler checks `state.config.solana.escrow_program_id` — return 404 if `None`.
4. Fetch or return cached slot (5s TTL).
5. Register `.route("/v1/escrow/config", get(routes::escrow::escrow_config))` in `build_router`.
6. Add to `routes/mod.rs`.

---

## 8.6: Escrow Monitoring

Track fee payer SOL balances and claim success/failure metrics.

### Files to Modify

- `crates/gateway/src/main.rs` — add fee payer pubkeys to balance monitor
- `crates/gateway/src/routes/escrow.rs` — add `/v1/escrow/health` endpoint (or extend existing `/health`)
- `crates/x402/src/escrow/claim_processor.rs` — expose in-memory counters

### Design

**Fee payer balance monitoring:**
- The `BalanceMonitor` already runs a periodic background task. Currently it only monitors `recipient_wallet`.
- Add all fee payer pool pubkeys to the monitor's wallet list.
- The monitor already logs warnings at thresholds (configurable `warn_threshold_sol` and `critical_threshold_sol`).

**Claim metrics (in-memory):**
- Add `ClaimMetrics` struct: `Arc<AtomicU64>` counters for `claims_submitted`, `claims_succeeded`, `claims_failed`, `claims_retried`.
- Updated by `claim_processor` on each outcome.
- Exposed via `/v1/escrow/health` or `/health` escrow subsection.

**Health endpoint response:**
```json
{
  "escrow_enabled": true,
  "claim_processor_running": true,
  "fee_payer_wallets": 3,
  "claims": {
    "submitted": 1520,
    "succeeded": 1498,
    "failed": 12,
    "retried": 10,
    "pending_in_queue": 3
  }
}
```

- `pending_in_queue` requires a DB query (`SELECT COUNT(*) FROM escrow_claim_queue WHERE status = 'pending'`). Only fetch when DB pool is available.

### Implementation Steps

1. In `main.rs`, extract fee payer pubkeys from `FeePayerPool` and add to `BalanceMonitor` wallet list.
2. Create `ClaimMetrics` struct in `claim_processor.rs` with atomic counters.
3. Pass `Arc<ClaimMetrics>` to `start_claim_processor` and update counters on each claim outcome.
4. Store `Arc<ClaimMetrics>` on `AppState`.
5. Add `GET /v1/escrow/health` handler (or add escrow section to existing `/health`).
6. Wire route in `build_router`.

---

## 8.7: Integration Tests

~15 new tests covering the escrow hardening features.

### Files to Modify

- `crates/gateway/tests/integration/escrow.rs` — new test module
- `crates/x402/src/escrow/claim_processor.rs` — unit tests for circuit breaker
- `crates/x402/src/fee_payer.rs` — additional tests for pool + claimer integration

### Test Plan

| # | Test | Type | Description |
|---|------|------|-------------|
| 1 | `test_claim_processor_graceful_shutdown` | Unit | Processor exits cleanly on shutdown signal |
| 2 | `test_claim_processor_drains_current_batch` | Unit | In-progress claims finish before shutdown |
| 3 | `test_fee_payer_rotation_on_claim_failure` | Unit | Failed claim marks wallet, next claim uses different wallet |
| 4 | `test_fee_payer_all_failed_returns_error` | Unit | All wallets in cooldown produces clear error |
| 5 | `test_nonce_fallback_to_blockhash` | Unit | When nonce pool is empty, falls back to blockhash with warning |
| 6 | `test_exponential_backoff_schedule` | Unit | Verify backoff durations: 1s, 2s, 4s... 300s cap |
| 7 | `test_circuit_breaker_opens_on_high_failure_rate` | Unit | >50% failures in window opens breaker |
| 8 | `test_circuit_breaker_closes_after_pause` | Unit | Breaker closes after 60s pause |
| 9 | `test_circuit_breaker_minimum_sample_size` | Unit | Breaker does not trip below 4 total claims |
| 10 | `test_max_retries_marks_failed` | Unit | Claim marked `failed` after 10 attempts |
| 11 | `test_escrow_config_endpoint_returns_config` | Integration | Returns 200 with escrow params when configured |
| 12 | `test_escrow_config_endpoint_returns_404` | Integration | Returns 404 when escrow not configured |
| 13 | `test_escrow_health_endpoint` | Integration | Returns claim metrics and fee payer count |
| 14 | `test_claim_metrics_increment` | Unit | Counters increment correctly on success/failure |
| 15 | `test_slot_cache_ttl` | Unit | Cached slot expires after 5 seconds |

### Implementation Steps

1. Add circuit breaker unit tests first (TDD).
2. Add backoff schedule unit tests.
3. Add shutdown signal tests.
4. Add integration tests for new endpoints using `tower::ServiceExt::oneshot`.
5. Verify all existing 342 tests still pass.

---

## Decision Log

| # | Decision | Alternatives Considered | Rationale |
|---|----------|------------------------|-----------|
| 80 | Keep single-signer constraint (fee_payer == recipient_wallet) for claims | Implement two-signer claim tx path | Two-signer requires restructuring the Anchor program's `Claim` accounts or co-signing with the recipient key. The single-signer path works when the operator controls the recipient wallet and uses it as fee payer. Avoids Anchor program changes in a hardening phase. |
| 81 | Shutdown via `tokio::sync::watch` channel | `CancellationToken`, `broadcast`, `oneshot` | Watch is cheapest for single-producer-multi-consumer bool signal. CancellationToken requires `tokio-util` dep. |
| 82 | Exponential backoff stored in DB (`next_retry_at` column) | In-memory backoff state | Survives gateway restarts. Claims already persisted in `escrow_claim_queue`. |
| 83 | Circuit breaker in-memory with 5-min rolling window | Per-claim backoff only, Redis-backed | In-memory is sufficient — circuit breaker is per-instance, not distributed. Resets on restart is acceptable (fail-open). |
| 84 | MAX_CLAIM_ATTEMPTS increased from 5 to 10 | Keep at 5, unlimited retries | 10 attempts with exponential backoff covers ~10 minutes of retries (1+2+4+8+16+32+64+128+256+300 = ~811s). Enough for transient RPC issues. |
| 85 | Slot cache on AppState with 5s TTL | No cache (per-request RPC), longer TTL | 5s balances freshness vs RPC cost. Slot advances every ~400ms so 5s is ~12 slots stale — acceptable for config discovery. |
| 86 | Escrow config as separate `/v1/escrow/config` endpoint | Add to existing `/health`, embed in 402 response | Dedicated endpoint is RESTful and discoverable. 402 already includes escrow_program_id in `PaymentAccept`. Health endpoint is for operational status, not configuration. |
| 87 | Claim metrics as atomic counters on AppState | PostgreSQL counters, Redis counters | Zero-cost in-memory atomics. Resets on restart is fine — these are operational metrics, not billing data. Persistent claim data is already in the DB. |
| 88 | Fee payer pubkeys added to existing BalanceMonitor | Separate escrow-specific monitor | BalanceMonitor already handles multi-wallet + threshold alerts. No reason to duplicate. |

---

## Build Order

```
8.4 ─────────────────────┐
  (backoff + circuit      │
   breaker, no deps)      │
                          ▼
8.2 ──► 8.3 ──► 8.1 ──► 8.6 ──► 8.7
(pool)  (nonce) (start)  (monitor) (tests)
                  │
                  ▼
                8.5
              (endpoint)
```

**Recommended order:**

1. **8.4** — Error recovery (backoff + circuit breaker). No external dependencies. Foundation for reliable claims.
2. **8.2** — Fee payer rotation. Refactor `EscrowClaimer` to use `FeePayerPool`.
3. **8.3** — Durable nonces. Depends on 8.2 (claimer refactor).
4. **8.1** — Claim processor auto-start with graceful shutdown. Depends on 8.2/8.3 for the final claimer signature.
5. **8.5** — Escrow config endpoint. Independent, can be done in parallel with 8.1-8.4.
6. **8.6** — Monitoring. Depends on 8.1 (processor running), 8.2 (pool for pubkeys), 8.4 (metrics counters).
7. **8.7** — Integration tests. Last — covers all features.

---

## Implementation Checklist

### 8.1: Claim Processor Auto-Start
- [x]Add shutdown channel parameter to `start_claim_processor`
- [x]Replace bare loop with `tokio::select!` on interval + shutdown
- [x]Wire shutdown signal in `main.rs`
- [x]Connect to `axum::serve` graceful shutdown
- [x]Test: processor exits on shutdown signal
- [x]Test: in-progress batch completes before exit

### 8.2: Fee Payer Rotation
- [x]Refactor `EscrowClaimer` to hold `Arc<FeePayerPool>` instead of single key
- [x]Update `do_claim` to call `pool.next()` for wallet selection
- [x]Mark wallet as failed on "insufficient lamports" RPC errors
- [x]Update constructor (`new`) signature
- [x]Update `main.rs` to pass pool to claimer
- [x]Update `Drop` impl
- [x]Test: rotation on failure
- [x]Test: all-failed returns clear error

### 8.3: Durable Nonces for Claims
- [x]Add `nonce_pool: Option<Arc<NoncePool>>` to `EscrowClaimer`
- [x]In `do_claim`, attempt nonce fetch before blockhash
- [x]Build `AdvanceNonce` instruction when using durable nonce
- [x]Fall back to blockhash with `tracing::warn!` on nonce failure
- [x]Pass nonce pool from `main.rs`
- [x]Test: fallback to blockhash when pool is empty

### 8.4: Error Recovery
- [x]Add migration: `next_retry_at` column on `escrow_claim_queue`
- [x]Update `fetch_pending_claims` query to filter by `next_retry_at`
- [x]Update `mark_attempt_failed` to compute exponential backoff delay
- [x]Increase `MAX_CLAIM_ATTEMPTS` to 10
- [x]Implement `ClaimCircuitBreaker` struct
- [x]Check circuit breaker state before each processing cycle
- [x]Add structured tracing for circuit open/close events
- [x]Test: backoff schedule correctness
- [x]Test: circuit breaker opens at >50% failure rate
- [x]Test: circuit breaker closes after 60s
- [x]Test: minimum sample size (4) prevents false trips

### 8.5: GET /v1/escrow/config
- [x]Create `crates/gateway/src/routes/escrow.rs`
- [x]Add `SlotCache` to `AppState`
- [x]Implement handler: check config, fetch/cache slot, return JSON
- [x]Return 404 when escrow not configured
- [x]Register route in `build_router`
- [x]Add to `routes/mod.rs`
- [x]Test: returns 200 with config when escrow enabled
- [x]Test: returns 404 when escrow disabled
- [x]Test: slot cache TTL works

### 8.6: Escrow Monitoring
- [x]Add fee payer pool pubkeys to `BalanceMonitor` wallet list in `main.rs`
- [x]Create `ClaimMetrics` struct with atomic counters
- [x]Pass metrics to `start_claim_processor`
- [x]Update processor to increment counters on success/failure/retry
- [x]Store `Arc<ClaimMetrics>` on `AppState`
- [x]Add `/v1/escrow/health` handler (or extend `/health`)
- [x]Include pending queue count (DB query) when pool available
- [x]Test: metrics increment correctly
- [x]Test: health endpoint returns expected shape

### 8.7: Integration Tests
- [x]Circuit breaker unit tests (TDD — write first)
- [x]Backoff schedule unit tests
- [x]Shutdown signal unit tests
- [x]Fee payer rotation unit tests
- [x]Escrow config endpoint integration test (oneshot)
- [x]Escrow health endpoint integration test (oneshot)
- [x]Verify all 342 existing tests still pass
- [x]Verify `cargo clippy --all-targets --all-features -- -D warnings` passes
