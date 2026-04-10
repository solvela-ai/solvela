# Plan Review: `rcr loadtest` — Load Testing Full Payment Path

**Date:** 2026-04-10
**Plan:** `2026-04-10-load-testing-full-payment-path.md`
**Reviewers:** architect, code-reviewer, security-reviewer, critic (parallel agents)

---

## Overall Verdict: ACCEPT WITH RESERVATIONS

The plan is well-structured, follows TDD rigorously across 10 tasks with 35+ tests, and fits cleanly into the existing CLI crate. However, there are **3 issues that should be fixed before implementation**.

---

## CRITICAL / HIGH Issues (fix before implementing)

### 1. Coordinated Omission Bug — Dispatcher Design (HIGH)

The plan claims to prevent coordinated omission, but the dispatcher loop (`interval.tick() -> semaphore.acquire() -> spawn`) **actually exhibits it**. When the semaphore is saturated, `acquire_owned` blocks the loop, ticks pile up, and Tokio's default `MissedTickBehavior::Burst` fires them all at once when a permit frees. Latency measurement starts *after* the semaphore wait, making queuing delay invisible.

**Fix options:**
- Record `Instant::now()` at `interval.tick()` and pass it to the worker as the latency start
- Set `interval.set_missed_tick_behavior(MissedTickBehavior::Skip)` + track dropped requests
- Use `try_acquire_owned` with a "dropped" counter for honest backpressure visibility

### 2. New `reqwest::Client` Per Payment Call (CRITICAL perf)

Both `ExactPaymentStrategy` and `EscrowPaymentStrategy` call `reqwest::Client::new()` inside `prepare_payment`. Under load, this creates thousands of TCP connections to Solana RPC with no connection reuse — risking FD exhaustion and port starvation.

**Fix:** Accept a shared `reqwest::Client` in each strategy's constructor.

### 3. Private Key Not Zeroed, No Debug Redaction (HIGH security)

Both payment strategies store `keypair_b58: String` as plain text. No `secrecy` crate wrapper means:
- Key leaks in debug output if anything logs `{:?}` on the strategy
- Key persists in heap memory after drop (no zeroization)

**Fix:** Wrap in `Secret<String>` from the `secrecy` crate.

---

## MEDIUM Issues (fix during or after implementation)

| Issue | Detail |
|-------|--------|
| **No 402-dance integration test** | Worker tests only use `DevBypassStrategy`. The 402 -> parse -> sign -> retry path is never tested end-to-end. Add a wiremock test returning 402 then 200. |
| **No TLS enforcement** | Signed Solana txns sent over whatever scheme `--api-url` uses. Warn loudly for `http://` in non-dev modes. |
| **`Vec::with_capacity(total_requests)`** | Pre-allocates 300K `JoinHandle`s at high RPS. Use `tokio::task::JoinSet` instead for incremental reaping. |
| **`record_error(RequestOutcome::Success)`** | Confusing API — `Success` variant in an error method. Remove it from `RequestOutcome` or make it unreachable. |
| **Wallet file permissions** | `load_wallet()` doesn't check file is `0600`. Add a permission check like SSH does. |

---

## LOW / Acceptable

- **`getrandom` for tier selection** — 1000 syscalls/sec is negligible on Linux; `fastrand` would be cleaner but not critical
- **`Ordering::Relaxed` on atomics** — correct; counters are independent and only read in aggregate snapshots
- **`Mutex<Histogram>`** — held for nanoseconds per record; not a bottleneck at this scale
- **Modulo bias in `rand_u8() % 100`** — negligible for load test tier selection
- **Dispatcher count test** — may be flaky under CI load; consider `assert!(9..=11)`

---

## Test Coverage: ~83%

| Module | Coverage | Gap |
|--------|----------|-----|
| Config | 95% | Missing `"0s"` and overflow edge cases |
| Metrics | 90% | Missing 0ms latency edge, `Success` in `record_error` |
| Payment strategies | 70% | **No test through worker 402-dance** |
| Worker | 60% | **402->sign->retry untested** |
| Dispatcher | 80% | Potential timing flakiness |
| Report + SLO | 95% | Well covered |
| Prometheus | 90% | Good |

---

## Recommended Action

Fix the **3 high/critical issues** in the plan before delegating to executors:

1. Redesign dispatcher to pass scheduled `Instant` into workers (coordinated omission)
2. Inject shared `reqwest::Client` into payment strategies (connection pooling)
3. Use `secrecy::Secret<String>` for keypair fields (key safety)

Then add the missing 402-dance integration test to Task 4 or Task 7. After those changes, the plan is ready for implementation.
