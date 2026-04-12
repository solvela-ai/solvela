# Load Testing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `--model` flag, live progress output, rate limit override, and Fly.io load runner — then execute all 7 test phases from the spec.

**Architecture:** 4 implementation tasks (CLI `--model` flag, live progress, gateway rate limit override, load runner infra), then 7 execution phases. Each implementation task is TDD.

**Tech Stack:** Rust (Axum, Tokio, clap, hdrhistogram), Docker, Fly.io CLI

---

### Task 1: Add `--model` Flag to Loadtest CLI

**Files:**
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (LoadTestArgs, build_request_body)
- Modify: `crates/cli/src/commands/loadtest/config.rs` (LoadTestConfig)

**Context:** Currently `build_request_body()` hardcodes `"auto"` for all tiers. Phase 7 needs `--model openai-gpt-4o-mini` to force a specific provider. The model string flows from CLI args → config → request body builder.

- [ ] **Step 1: Add `model` field to `LoadTestArgs`**

  In `crates/cli/src/commands/loadtest/mod.rs`, add to `LoadTestArgs`:
  ```rust
  /// Model to use for requests (default: "auto" for smart routing).
  #[arg(long, default_value = "auto")]
  pub model: String,
  ```

- [ ] **Step 2: Add `model` field to `LoadTestConfig`**

  In `crates/cli/src/commands/loadtest/config.rs`, add to `LoadTestConfig`:
  ```rust
  pub model: String,
  ```

- [ ] **Step 3: Wire model through `into_config`**

  In `mod.rs`, in `LoadTestArgs::into_config()`, add `model: self.model` to the `LoadTestConfig` construction (after the `dry_run` field).

- [ ] **Step 4: Pass model to `build_request_body`**

  Change `build_request_body(tier: &str)` signature to `build_request_body(tier: &str, model: &str)`.

  Update body:
  ```rust
  fn build_request_body(tier: &str, model: &str) -> serde_json::Value {
      let prompt = match tier {
          "simple" => "Say hello.",
          "medium" => "Explain how HTTP caching works with ETags and Cache-Control headers.",
          "complex" => "Write a Rust function that implements a lock-free concurrent hash map with linear probing. Include detailed comments explaining the memory ordering constraints.",
          "reasoning" => "Prove that every continuous function on a closed interval is uniformly continuous. Then explain why this fails for open intervals with a concrete counterexample.",
          _ => "Say hello.",
      };

      serde_json::json!({
          "model": model,
          "messages": [{"role": "user", "content": prompt}],
          "max_tokens": 64
      })
  }
  ```

- [ ] **Step 5: Update call site in `run()`**

  In the closure passed to `run_dispatcher`, change:
  ```rust
  let body = build_request_body(tier);
  ```
  to:
  ```rust
  let body = build_request_body(tier, &model);
  ```

  Add `let model: Arc<str> = Arc::from(config.model.as_str());` alongside the other `Arc` allocations, and clone it into the closure.

- [ ] **Step 6: Update dry-run output**

  Add `println!("Model:        {}", config.model);` to the dry-run block.

- [ ] **Step 7: Update integration tests**

  Add `model` field to all `LoadTestArgs` construction in integration tests:
  ```rust
  model: "auto".to_string(),
  ```

- [ ] **Step 8: Run tests**

  Run: `cargo test -p solvela-cli -- --nocapture`
  Expected: All existing + new tests pass.

- [ ] **Step 9: Commit**

  ```bash
  git add crates/cli/
  git commit -m "feat(cli): add --model flag to loadtest command"
  ```

---

### Task 2: Add Live Progress Output

**Files:**
- Modify: `crates/cli/src/commands/loadtest/dispatcher.rs`
- Modify: `crates/cli/src/commands/loadtest/metrics.rs` (add `snapshot_live` method)

**Context:** The dispatcher runs silently until completion. Add a tokio task that prints a progress line every second: `[15s/60s] RPS: 98.2 | OK: 1412 | 4xx: 23 | 5xx: 0 | drop: 0 | p99: 342ms`

- [ ] **Step 1: Add `snapshot` method that doesn't need ownership**

  The existing `snapshot()` method on `MetricsCollector` already takes `&self` and returns `MetricsSnapshot`. Confirmed — no changes needed to metrics.rs.

- [ ] **Step 2: Add progress printer to dispatcher**

  In `dispatcher.rs`, after creating the `join_set` and before the dispatch loop, spawn a progress printer task:

  ```rust
  // Live progress output — prints metrics every second.
  let progress_metrics = metrics.clone();
  let total_duration = Duration::from_secs(config.duration_secs);
  let progress_handle = tokio::spawn(async move {
      let start = Instant::now();
      let mut tick = time::interval(Duration::from_secs(1));
      tick.tick().await; // skip immediate first tick
      loop {
          tick.tick().await;
          let elapsed = start.elapsed();
          if elapsed >= total_duration + Duration::from_secs(5) {
              break; // safety exit
          }
          let snap = progress_metrics.snapshot();
          let elapsed_secs = elapsed.as_secs();
          let duration_secs = total_duration.as_secs();
          let effective_rps = if elapsed_secs > 0 {
              snap.total_requests as f64 / elapsed_secs as f64
          } else {
              0.0
          };
          eprint!(
              "\r[{elapsed_secs}s/{duration_secs}s] RPS: {effective_rps:.1} | OK: {} | 4xx: {} | 5xx: {} | drop: {} | p99: {}ms   ",
              snap.successful,
              snap.payment_required_402 + snap.rate_limited_429,
              snap.server_errors_5xx,
              snap.dropped_requests,
              snap.p99_ms,
          );
      }
  });
  ```

  After the dispatch loop completes and all join_set tasks finish, abort the progress printer and print a final newline:

  ```rust
  progress_handle.abort();
  eprintln!(); // newline after progress line
  ```

- [ ] **Step 3: Add `duration_secs` to `DispatcherConfig`**

  The progress printer needs the total duration. It's already in `DispatcherConfig` — confirmed. No change needed.

  Wait — check: `DispatcherConfig` has `duration_secs: u64`. Confirmed. Good.

- [ ] **Step 4: Run tests**

  Run: `cargo test -p solvela-cli -- --nocapture`
  Expected: All tests pass. Progress output will appear in test output (harmless).

- [ ] **Step 5: Commit**

  ```bash
  git add crates/cli/src/commands/loadtest/dispatcher.rs
  git commit -m "feat(cli): add live progress output to loadtest dispatcher"
  ```

---

### Task 3: Add Rate Limit Env Var Override

**Files:**
- Modify: `crates/gateway/src/middleware/rate_limit.rs` (RateLimitConfig)
- Modify: `crates/gateway/src/main.rs` (read env var)

**Context:** Default rate limit is 60 req/60s. High-RPS load tests hit this immediately. Add `SOLVELA_RATE_LIMIT_MAX` env var to override.

- [ ] **Step 1: Write failing test**

  In `crates/gateway/src/middleware/rate_limit.rs`, add test:
  ```rust
  #[test]
  fn test_rate_limit_config_from_env_override() {
      let config = RateLimitConfig::with_max_requests(10000);
      assert_eq!(config.max_requests, 10000);
      assert_eq!(config.unknown_max_requests, 10); // unchanged
  }
  ```

- [ ] **Step 2: Implement `with_max_requests`**

  Add to `RateLimitConfig`:
  ```rust
  /// Create a config with a custom max_requests value.
  /// Used for env var override during load testing.
  pub fn with_max_requests(max: u32) -> Self {
      Self {
          max_requests: max,
          ..Self::default()
      }
  }
  ```

- [ ] **Step 3: Run test**

  Run: `cargo test -p gateway test_rate_limit_config_from_env_override`
  Expected: PASS

- [ ] **Step 4: Wire env var in main.rs**

  In `crates/gateway/src/main.rs`, find where `RateLimiter::new(RateLimitConfig::default())` is called. Replace with:

  ```rust
  let rate_limit_config = match env_with_fallback("SOLVELA_RATE_LIMIT_MAX", "RCR_RATE_LIMIT_MAX") {
      Ok(val) => {
          let max: u32 = val.parse().unwrap_or_else(|_| {
              warn!("Invalid SOLVELA_RATE_LIMIT_MAX value '{val}', using default");
              60
          });
          tracing::info!(max_requests = max, "Rate limit override from env");
          RateLimitConfig::with_max_requests(max)
      }
      Err(_) => RateLimitConfig::default(),
  };
  let rate_limiter = RateLimiter::new(rate_limit_config);
  ```

- [ ] **Step 5: Run full gateway tests**

  Run: `cargo test -p gateway`
  Expected: All tests pass.

- [ ] **Step 6: Commit**

  ```bash
  git add crates/gateway/src/middleware/rate_limit.rs crates/gateway/src/main.rs
  git commit -m "feat(gateway): add SOLVELA_RATE_LIMIT_MAX env var override for load testing"
  ```

---

### Task 4: Load Runner Infrastructure

**Files:**
- Create: `loadtest/Dockerfile`
- Create: `loadtest/fly.toml`
- Create: `loadtest/run.sh`

**Context:** A minimal Fly.io app in ord region that runs the `solvela` CLI binary for co-located load testing.

- [ ] **Step 1: Create loadtest directory**

  ```bash
  mkdir -p loadtest
  ```

- [ ] **Step 2: Create Dockerfile**

  `loadtest/Dockerfile`:
  ```dockerfile
  FROM debian:bookworm-slim
  RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
  COPY solvela /usr/local/bin/solvela
  RUN chmod +x /usr/local/bin/solvela
  ENTRYPOINT ["sleep", "infinity"]
  ```

  Note: `sleep infinity` keeps the machine running so we can `fly ssh console` into it and run commands interactively. The binary is built locally and copied in.

- [ ] **Step 3: Create fly.toml**

  `loadtest/fly.toml`:
  ```toml
  app = "solvela-loadtest-runner"
  primary_region = "ord"

  [build]

  [[vm]]
  size = "performance-1x"
  memory = "512mb"
  ```

- [ ] **Step 4: Create runner script**

  `loadtest/run.sh`:
  ```bash
  #!/usr/bin/env bash
  set -euo pipefail

  API_URL="${API_URL:-https://solvela-gateway.fly.dev}"
  RESULTS_DIR="/tmp/loadtest-results"
  mkdir -p "$RESULTS_DIR"

  echo "=== Phase 1: Baseline (shared-cpu-1x) ==="
  for rps in 10 50 100 200; do
      echo "--- ${rps} RPS ---"
      solvela loadtest \
          --api-url "$API_URL" \
          --rps "$rps" \
          --duration 60s \
          --concurrency "$((rps * 2))" \
          --mode dev-bypass \
          --slo-p99-ms 10000 \
          --slo-error-rate 0.50 \
          --report-json "$RESULTS_DIR/phase1-${rps}rps.json" \
          || true
      echo ""
      sleep 5
  done

  echo "=== Phase 2: Break-point ==="
  rps=200
  while true; do
      rps=$((rps + 50))
      echo "--- ${rps} RPS (break-point ramp) ---"
      solvela loadtest \
          --api-url "$API_URL" \
          --rps "$rps" \
          --duration 60s \
          --concurrency "$((rps * 2))" \
          --mode dev-bypass \
          --slo-p99-ms 10000 \
          --slo-error-rate 0.10 \
          --report-json "$RESULTS_DIR/phase2-${rps}rps.json"

      if [ $? -ne 0 ]; then
          echo "Break-point reached at ${rps} RPS"
          break
      fi

      if [ "$rps" -ge 2000 ]; then
          echo "Reached 2000 RPS cap without breaking"
          break
      fi
      sleep 10
  done

  echo "=== Results ==="
  ls -la "$RESULTS_DIR/"
  echo "Copy results with: fly ssh sftp get $RESULTS_DIR/*.json"
  ```

- [ ] **Step 5: Commit**

  ```bash
  git add loadtest/
  git commit -m "feat: add Fly.io load runner infrastructure"
  ```

---

### Task 5: Build and Deploy Load Runner

**Prerequisite:** Tasks 1-3 complete and pushed to main.

- [ ] **Step 1: Build release binary**

  ```bash
  cargo build --release -p solvela-cli
  ```

- [ ] **Step 2: Copy binary to loadtest dir**

  ```bash
  cp target/release/solvela loadtest/solvela
  ```

- [ ] **Step 3: Deploy to Fly.io**

  ```bash
  cd loadtest
  fly launch --no-deploy --copy-config
  fly deploy
  ```

- [ ] **Step 4: Set secrets on runner**

  ```bash
  fly secrets set SOLANA_WALLET_KEY="<wallet-key>" -a solvela-loadtest-runner
  fly secrets set SOLANA_RPC_URL="<rpc-url>" -a solvela-loadtest-runner
  ```

- [ ] **Step 5: Verify runner is up**

  ```bash
  fly ssh console -a solvela-loadtest-runner -C "solvela --help"
  ```

---

### Task 6: Configure Gateway for Testing

- [ ] **Step 1: Temporarily unset production env**

  ```bash
  fly secrets unset SOLVELA_ENV -a solvela-gateway
  ```

- [ ] **Step 2: Enable dev-bypass**

  ```bash
  fly secrets set SOLVELA_DEV_BYPASS_PAYMENT=true -a solvela-gateway
  ```

- [ ] **Step 3: Set rate limit override**

  ```bash
  fly secrets set SOLVELA_RATE_LIMIT_MAX=10000 -a solvela-gateway
  ```

- [ ] **Step 4: Verify gateway health**

  ```bash
  curl -s https://solvela-gateway.fly.dev/health | jq .
  ```

---

### Task 7: Execute Phase 1-2 (Baseline + Break-point, T1)

- [ ] **Step 1: Run Phase 1 from runner**

  ```bash
  fly ssh console -a solvela-loadtest-runner
  # Inside runner:
  bash /loadtest/run.sh  # or run commands manually
  ```

  Alternatively run each step individually via `fly ssh console -C`:
  ```bash
  fly ssh console -a solvela-loadtest-runner -C \
    "solvela loadtest --api-url https://solvela-gateway.fly.dev --rps 10 --duration 60s --concurrency 20 --mode dev-bypass --report-json /tmp/phase1-10rps.json"
  ```

- [ ] **Step 2: Monitor from local**

  In a separate terminal:
  ```bash
  fly logs -a solvela-gateway
  ```

- [ ] **Step 3: Collect and review Phase 1 results**

  Download JSON reports, review p50/p95/p99 at each RPS level. Note first signs of degradation.

- [ ] **Step 4: Run Phase 2 break-point ramp**

  Continue ramping +50 RPS per step until SLO fails.

- [ ] **Step 5: Record T1 ceiling**

  Document the max healthy RPS for shared-cpu-1x.

---

### Task 8: Execute Phase 3-4 (Scaled Tiers)

- [ ] **Step 1: Scale to T2**

  ```bash
  fly scale vm performance-2x -a solvela-gateway
  ```

- [ ] **Step 2: Run Phase 3 (same pattern as Phase 1-2 but with higher RPS targets)**

- [ ] **Step 3: Record T2 ceiling**

- [ ] **Step 4: Scale to T3**

  ```bash
  fly scale vm dedicated-cpu-1x --memory 2048 -a solvela-gateway
  ```

- [ ] **Step 5: Run Phase 4 (ramp to 1000+ RPS)**

- [ ] **Step 6: Record T3 ceiling**

- [ ] **Step 7: Scale back to production tier**

  ```bash
  fly scale vm shared-cpu-1x --memory 512 -a solvela-gateway
  ```

---

### Task 9: Execute Phase 5 (SLO Validation)

- [ ] **Step 1: Choose target RPS and tier based on Phase 1-4 findings**

- [ ] **Step 2: Run 5-minute sustained load**

  ```bash
  solvela loadtest \
      --api-url https://solvela-gateway.fly.dev \
      --rps <target> \
      --duration 300s \
      --concurrency <2x target> \
      --mode dev-bypass \
      --slo-p99-ms 5000 \
      --slo-error-rate 0.01 \
      --prometheus-url "https://solvela-gateway.fly.dev/metrics" \
      --report-json /tmp/phase5-slo.json
  ```

- [ ] **Step 3: Record pass/fail**

---

### Task 10: Execute Phase 6-7 (Payment + Provider Verification)

**Cost: ~$6-15 USDC. Confirm with user before proceeding.**

- [ ] **Step 1: Restore production env (payment tests need real verification)**

  ```bash
  fly secrets set SOLVELA_ENV=production -a solvela-gateway
  fly secrets unset SOLVELA_DEV_BYPASS_PAYMENT -a solvela-gateway
  fly secrets unset SOLVELA_RATE_LIMIT_MAX -a solvela-gateway
  ```

- [ ] **Step 2: Run Phase 6 exact payment (from local)**

  ```bash
  SOLANA_WALLET_KEY="<key>" SOLANA_RPC_URL="<url>" \
  solvela loadtest \
      --api-url https://solvela-gateway.fly.dev \
      --rps 5 --duration 60s --concurrency 10 \
      --mode exact \
      --report-json docs/load-tests/results/phase6-exact.json
  ```

- [ ] **Step 3: Run Phase 6 escrow payment (from local)**

  ```bash
  SOLANA_WALLET_KEY="<key>" SOLANA_RPC_URL="<url>" \
  solvela loadtest \
      --api-url https://solvela-gateway.fly.dev \
      --rps 2 --duration 30s --concurrency 5 \
      --mode escrow \
      --report-json docs/load-tests/results/phase6-escrow.json
  ```

- [ ] **Step 4: Run Phase 7 per-provider verification (from local)**

  ```bash
  for model in openai-gpt-4o-mini anthropic-claude-haiku-4-5 google-gemini-2-0-flash xai-grok-3-mini deepseek-chat; do
      echo "--- Testing $model ---"
      SOLANA_WALLET_KEY="<key>" SOLANA_RPC_URL="<url>" \
      solvela loadtest \
          --api-url https://solvela-gateway.fly.dev \
          --rps 2 --duration 30s --concurrency 5 \
          --mode exact --model "$model" \
          --report-json "docs/load-tests/results/phase7-${model}.json"
      sleep 5
  done
  ```

- [ ] **Step 5: Review results and verify all providers work**

---

### Task 11: Teardown and Report

- [ ] **Step 1: Destroy load runner**

  ```bash
  fly apps destroy solvela-loadtest-runner -y
  ```

- [ ] **Step 2: Verify gateway is back to normal**

  ```bash
  fly secrets set SOLVELA_ENV=production -a solvela-gateway  # already set in Task 10
  curl -s https://solvela-gateway.fly.dev/health | jq .
  ```

- [ ] **Step 3: Write summary report**

  Create `docs/load-tests/2026-04-12-results.md` with:
  - Comparison table across T1/T2/T3 (RPS ceiling, p50/p95/p99 at each level)
  - Scaling recommendation
  - Provider verification results
  - Any bugs or issues found
  - SLO pass/fail

- [ ] **Step 4: Commit results**

  ```bash
  git add docs/load-tests/
  git commit -m "docs: add load test results and scaling recommendations"
  ```
