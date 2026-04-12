# Load Testing Design — Solvela Gateway

> **Goal:** Establish baseline performance, find the ceiling on each machine tier, validate SLOs, and verify all payment paths (exact + escrow) across all 5 LLM providers under load.

## Context

The Solvela gateway (`solvela-gateway.fly.dev`) has never been load tested against production. A CLI load test framework (`solvela loadtest`) was built on Apr 10 with constant-arrival-rate dispatch, HDR histogram latency tracking, 3 payment strategies (dev-bypass, exact, escrow), Prometheus integration, and SLO validation. The gateway runs on Fly.io shared-cpu-1x/512MB in ord (Chicago) with PostgreSQL and Redis.

No real user traffic exists yet — RustyClaw Terminal and Telsi.ai are not yet sending production requests through the gateway. This is a clean-room test.

## Infrastructure

### Load Runner (Temporary Fly.io Machine)

A temporary Fly.io app in the same `ord` region running the `solvela` CLI binary. Near-zero network latency to the gateway eliminates local machine/WSL2 as a bottleneck for high-RPS phases.

- **App name:** `solvela-loadtest-runner` (temporary, torn down after testing)
- **Machine:** performance-1x (~$0.03/hr)
- **Region:** ord (co-located with gateway)
- **Image:** Minimal Dockerfile with `solvela` binary + a runner script
- **Secrets:** `SOLANA_WALLET_KEY`, `SOLANA_RPC_URL`, `SOLVELA_ADMIN_TOKEN` (for Prometheus scraping)
- **Output:** JSON reports written to stdout, captured via `fly logs`

### Local Machine (WSL2, 1Gbps fiber)

Used for payment-path tests at low RPS (2-5) where network latency is part of the real-world experience. Also used for monitoring via `fly logs`.

### Gateway Scaling Plan

Tests run against three machine tiers, scaling in place:

| Tier | Fly.io VM | Cost | Purpose |
|------|-----------|------|---------|
| T1 | shared-cpu-1x / 512MB | Current (no extra) | Production baseline |
| T2 | performance-2x / 1GB | ~$0.06/hr | Mid-tier headroom |
| T3 | dedicated-cpu-1x / 2GB | ~$0.07/hr | Ceiling discovery |

Scale back to T1 after testing.

## Test Protocol

**Warmup:** Every phase begins with a 10s warmup at 5 RPS (not recorded) to prime connection pools, Tokio runtime, and JIT effects.

**Cooldown:** 30s pause between phases (or verify `/health` returns 200 within 100ms) to ensure the gateway has recovered from any prior degradation.

**Duration:** Minimum 60s per step for statistical significance (600+ data points at 10 RPS; p99 from 300 samples at 30s is too noisy).

**Memory monitoring:** Check gateway RSS via `fly machine status -a solvela-gateway` between phases. If RSS > 400MB on T1 (512MB), note OOM risk.

## Test Phases

### Phase 1: Baseline (T1, dev-bypass, $0)

**Runner:** Fly.io  
**Goal:** Establish performance profile on current production hardware.

| Step | RPS | Duration | Concurrency |
|------|-----|----------|-------------|
| 1a | 10 | 60s | 20 |
| 1b | 50 | 60s | 50 |
| 1c | 100 | 60s | 100 |
| 1d | 200 | 60s | 200 |

Tier weights: `simple=40,medium=30,complex=20,reasoning=10`

**Collect:** p50/p95/p99 latency, error rate, 429 rate, dropped requests per step. JSON report per step.

### Phase 2: Break-point (T1, dev-bypass, $0)

**Runner:** Fly.io  
**Goal:** Find the RPS ceiling where the shared-cpu-1x machine degrades.

Continue ramping from Phase 1's last stable RPS in +50 RPS increments, 60s each. Stop when:
- Error rate > 10%, OR
- p99 > 10s, OR
- Dropped requests > 5%

Record the last "healthy" RPS as T1's ceiling.

**Known confounder:** The rate limiter uses `tokio::sync::Mutex<HashMap>` — at high concurrency this lock becomes a bottleneck. Phase 1-2 results include rate limiter mutex contention overhead. The rate limit override (see Implementation section) raises the limit but doesn't eliminate the lock. Results should be interpreted as "gateway + rate limiter" performance, not pure routing performance.

### Phase 3: Scaled Baseline (T2, dev-bypass, ~$0.06/hr)

**Runner:** Fly.io  
**Goal:** Measure headroom on performance-2x.

Scale gateway: `fly scale vm performance-2x -a solvela-gateway`

| Step | RPS | Duration | Concurrency |
|------|-----|----------|-------------|
| 3a | 10 | 60s | 20 |
| 3b | 100 | 60s | 100 |
| 3c | 200 | 60s | 200 |
| 3d | 500 | 60s | 500 |

Then ramp to break-point same as Phase 2.

### Phase 4: Ceiling Discovery (T3, dev-bypass, ~$0.07/hr)

**Runner:** Fly.io  
**Goal:** Find the absolute ceiling on dedicated-cpu-1x.

Scale gateway: `fly scale vm dedicated-cpu-1x --memory 2048 -a solvela-gateway`

| Step | RPS | Duration | Concurrency |
|------|-----|----------|-------------|
| 4a | 10 | 60s | 20 |
| 4b | 100 | 60s | 100 |
| 4c | 500 | 60s | 500 |
| 4d | 1000 | 60s | 1000 |

Then ramp to break-point.

### Phase 5: SLO Validation (best tier, dev-bypass, $0)

**Runner:** Fly.io  
**Goal:** Sustained load at target production RPS for 5 minutes.

Use the machine tier and RPS that best matches expected production load (determined by Phase 1-4 findings). Run for 300s with SLO enforcement:

- `--slo-p99-ms 5000`
- `--slo-error-rate 0.01`
- `--prometheus-url` pointed at gateway metrics endpoint

Pass/fail determines production readiness.

### Phase 6: Payment Path — Auto Mode (~$3-5 USDC)

**Runner:** Local machine  
**Goal:** Validate full 402 dance with real Solana payments.

**Exact payment:**
```
solvela loadtest --rps 5 --duration 60s --mode exact --concurrency 10
```

**Escrow payment:**
```
solvela loadtest --rps 2 --duration 30s --mode escrow --concurrency 5
```

Tier weights default (auto routing). Verify:
- All requests complete the 402 → sign → retry → 200 flow
- USDC transfers appear on-chain
- Escrow deposits are claimable
- Gateway spend logs match actual transfers

### Phase 7: Provider Verification (~$2-5 USDC)

**Runner:** Local machine  
**Goal:** Confirm each LLM provider works end-to-end with real payment.

5 separate runs at 2 RPS for 30s, each forcing a specific model (IDs from `config/models.toml`):
- OpenAI: `openai-gpt-4o-mini`
- Anthropic: `anthropic-claude-haiku-4-5`
- Google: `google-gemini-2-0-flash`
- xAI: `xai-grok-3-mini`
- DeepSeek: `deepseek-chat`

Requires the `--model` flag addition (see Implementation section).

## Implementation: Live Progress Output

Add a real-time progress line to the dispatcher that updates every second:

```
[15s/30s] RPS: 98.2 | OK: 1412 | 4xx: 23 | 5xx: 0 | drop: 0 | p99: 342ms
```

Implementation:
- Spawn a tokio task in the dispatcher that reads from MetricsCollector every 1s
- Print using `\r` carriage return (overwrites the line)
- Final newline when test completes, before the full report

## Implementation: Load Runner Dockerfile

Minimal container for the Fly.io load runner:

```dockerfile
FROM debian:bookworm-slim
COPY solvela /usr/local/bin/solvela
ENTRYPOINT ["solvela"]
```

Build the `solvela` binary locally with `cargo build --release -p solvela-cli`, copy to a temp directory, build the Docker image, deploy to Fly.io.

Runner fly.toml:
```toml
app = "solvela-loadtest-runner"
primary_region = "ord"

[build]

[[vm]]
size = "performance-1x"
memory = "512mb"
```

The runner is invoked via `fly ssh console` or `fly machine run` with the loadtest arguments.

## Implementation: Model Override for Provider Tests

The loadtest command currently hardcodes `model: "auto"`. For Phase 7 (per-provider verification), add a `--model` flag:

```rust
#[arg(long, default_value = "auto")]
model: String,
```

Wire it through to the request body builder in the worker.

## Implementation: Dev-Bypass in Production

The gateway blocks dev-bypass when `SOLVELA_ENV=production` (or `RCR_ENV=production`). This is checked at startup in `main.rs:158-161`. For Phases 1-5 to work against the Fly.io deployment:

**Option A (recommended):** Temporarily unset `SOLVELA_ENV` on Fly.io during testing, then re-set after:
```bash
fly secrets unset SOLVELA_ENV -a solvela-gateway   # enables dev-bypass
# ... run Phases 1-5 ...
fly secrets set SOLVELA_ENV=production -a solvela-gateway  # restore
```

**Option B:** Deploy a separate `solvela-gateway-test` app without the production env var. More isolated but more setup.

Since no real users are on the gateway right now, Option A is safe and simple.

## Implementation: Rate Limit Adjustment

Current rate limit: 60 req/60s per client (wallet or IP). For high-RPS dev-bypass tests, the load runner will hit rate limits immediately at >1 RPS.

Options:
1. **Exempt dev-bypass requests from rate limiting** — Check for dev-bypass header and skip rate limit middleware
2. **Temporary config override** — Increase rate limit to 10,000/60s during testing
3. **Add a `--rate-limit-override` env var** — Gateway reads an override from env, defaults to normal

Recommend option 3: `SOLVELA_RATE_LIMIT_MAX=10000` env var, easy to set/unset on Fly.io.

## SLO Targets

| Metric | Target | Break-point |
|--------|--------|-------------|
| p50 latency | < 500ms | n/a |
| p95 latency | < 2s | n/a |
| p99 latency | < 5s | > 10s |
| Error rate | < 1% | > 10% |
| 429 rate | < 5% | > 20% |
| Dropped requests | 0% | > 5% |

These targets are provisional — Phase 1 baseline data may cause us to adjust them.

## Deliverables

1. **JSON reports** — One per phase/step, stored in `docs/load-tests/results/`
2. **Comparison table** — Markdown summary comparing all three machine tiers
3. **Scaling recommendation** — "At X concurrent users, scale to tier Y"
4. **Provider verification results** — Per-provider pass/fail with latency
5. **Bug list** — Any issues discovered under load
6. **Live progress output** — Added to CLI for real-time monitoring

## Estimated Costs

| Item | Cost |
|------|------|
| Fly.io load runner (~2 hrs) | ~$0.06 |
| Gateway T2 scaling (~1 hr) | ~$0.06 |
| Gateway T3 scaling (~1 hr) | ~$0.07 |
| Phase 6 USDC (exact + escrow) | ~$4-8 |
| Phase 7 USDC (5 providers) | ~$2-5 |
| Solana tx fees | ~$0.05 |
| **Total** | **~$6-20** |

## Monitoring During Tests

- **Live progress line** in CLI (updated every second)
- **`fly logs -a solvela-gateway`** in separate terminal for gateway-side visibility
- **Prometheus scraping** — Pre/post delta comparison per phase
- **Results shared phase by phase** so the user can steer (skip/adjust phases based on findings)

## Known Confounders

1. **Rate limiter mutex** — `tokio::sync::Mutex<HashMap>` contention inflates p99 at high concurrency. Results measure "gateway + rate limiter" not pure routing.
2. **Dev-bypass skips payment path** — Phases 1-5 don't exercise Solana RPC, facilitator verification, or Redis replay protection. SLO targets from dev-bypass are routing/proxy performance only.
3. **Fire-and-forget DB writes** — At 500+ RPS, `tokio::spawn` tasks for PostgreSQL writes may queue up. Monitor PG pool utilization between phases.
4. **Redis untested at scale** — Dev-bypass skips Redis (no replay protection). Phases 6-7 at low RPS won't stress it. Redis contention under load is not tested by this plan.

## What We Need Before Starting

- [ ] Funded Solana wallet with ~$25 USDC + 0.1 SOL for tx fees (for Phases 6-7)
- [ ] `SOLVELA_ADMIN_TOKEN` value (for Prometheus access)
- [ ] Confirmation that the current gateway deployment is up-to-date with latest code
- [ ] Temporarily unset `SOLVELA_ENV` on Fly.io (enables dev-bypass for Phases 1-5)
- [ ] Set `SOLVELA_DEV_BYPASS_PAYMENT=true` on Fly.io
- [ ] Set `SOLVELA_RATE_LIMIT_MAX=10000` on Fly.io (requires implementation first)
- [ ] User available to monitor/approve cost-bearing phases
- [ ] Post-test: restore `SOLVELA_ENV=production`, unset dev-bypass and rate limit override
