# Load Testing CLI with Full Payment-Path Coverage

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add an `rcr loadtest` CLI subcommand that exercises the RustyClawRouter gateway under realistic constant-arrival-rate load with real Solana payment transactions (exact SPL TransferChecked and escrow deposit/claim), catching concurrency bugs that unit tests miss.

**Architecture:** Constant-arrival-rate dispatcher (tokio interval + semaphore) sends N requests/sec regardless of response time, preventing coordinated omission bias. Each request worker performs the full 402 dance (send -> get 402 -> sign payment -> resubmit with header). Three modes: dev-bypass (no payment), exact (SPL TransferChecked), escrow (Anchor deposit). Thread-safe atomic counters + HdrHistogram collect latency percentiles. Terminal + JSON report output with optional Prometheus SLO validation.

**Tech Stack:** Rust, Tokio, clap (derive), reqwest, hdrhistogram, secrecy, ed25519-dalek, x402 crate (escrow deposit builder, Solana types), serde_json, chrono

**Skills to invoke:** @rustyclaw-orchestration, @solana-dev, @api-patterns, @domain-web, @rust-async-patterns

---

## File Structure

```
crates/cli/src/commands/loadtest/
  mod.rs           -- LoadTestArgs (clap derive), run() entry point, subcommand wiring
  config.rs        -- LoadTestConfig (validated from args + env), TierWeights, SloThresholds
  dispatcher.rs    -- ConstantArrivalRateDispatcher: tokio::time::interval + Semaphore
  worker.rs        -- Single request lifecycle: 402 dance, sign, resubmit, measure latency
  payment.rs       -- PaymentStrategy trait + DevBypass / ExactPayment / EscrowPayment impls
  metrics.rs       -- MetricsCollector: AtomicU64 counters, Arc<Mutex<Histogram>> for latency
  report.rs        -- Terminal table (colored) + JSON file report formatters
  prometheus.rs    -- /metrics scraper, delta computation, SLO validation
```

**Files modified:**
- `crates/cli/src/commands/mod.rs` -- add `pub mod loadtest;`
- `crates/cli/src/main.rs` -- add `Loadtest` variant to `Commands` enum + match arm
- `crates/cli/Cargo.toml` -- add `hdrhistogram = "7"` dependency

**Existing code reused (not modified):**
- `crates/cli/src/commands/solana_tx.rs` -- `fetch_blockhash`, `fetch_current_slot`, `build_usdc_transfer`, `build_escrow_deposit`
- `crates/cli/src/commands/chat.rs` -- `select_payment_scheme` pattern (reimplemented locally since it's private), `generate_service_id` pattern
- `crates/cli/src/commands/wallet.rs` -- `load_wallet()`
- `crates/x402/src/escrow/deposit.rs` -- `build_deposit_tx`, `DepositParams`
- `crates/protocol/src/payment.rs` -- `PaymentRequired`, `PaymentAccept`, `PaymentPayload`, `CostBreakdown`

---

## Task 1: Scaffold Module Structure and CLI Wiring

**Files:**
- Create: `crates/cli/src/commands/loadtest/mod.rs`
- Create: `crates/cli/src/commands/loadtest/config.rs`
- Modify: `crates/cli/src/commands/mod.rs`
- Modify: `crates/cli/src/main.rs`
- Modify: `crates/cli/Cargo.toml`

This task creates the empty module skeleton, wires the `loadtest` subcommand into clap, and adds the `hdrhistogram` dependency. The subcommand parses all arguments but prints "not yet implemented" and exits.

- [ ] **Step 1: Write the failing test for config validation**

In `crates/cli/src/commands/loadtest/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier_weights_default_sums_to_100() {
        let weights = TierWeights::default();
        let sum = weights.simple + weights.medium + weights.complex + weights.reasoning;
        assert_eq!(sum, 100, "default tier weights must sum to 100");
    }

    #[test]
    fn test_tier_weights_parse_valid() {
        let input = "simple=40,medium=30,complex=20,reasoning=10";
        let weights = TierWeights::parse(input).expect("valid weights");
        assert_eq!(weights.simple, 40);
        assert_eq!(weights.medium, 30);
        assert_eq!(weights.complex, 20);
        assert_eq!(weights.reasoning, 10);
    }

    #[test]
    fn test_tier_weights_parse_bad_sum() {
        let input = "simple=50,medium=50,complex=50,reasoning=50";
        let result = TierWeights::parse(input);
        assert!(result.is_err(), "weights summing to 200 should be rejected");
    }

    #[test]
    fn test_slo_thresholds_default() {
        let slo = SloThresholds::default();
        assert!(slo.p99_ms > 0, "default p99 SLO must be positive");
        assert!(slo.error_rate > 0.0 && slo.error_rate < 1.0);
    }

    #[test]
    fn test_load_test_config_validate_rejects_zero_rps() {
        let config = LoadTestConfig {
            api_url: "http://localhost:8402".to_string(),
            rps: 0,
            duration_secs: 60,
            concurrency: 10,
            mode: LoadTestMode::DevBypass,
            tier_weights: TierWeights::default(),
            slo: SloThresholds::default(),
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };
        assert!(config.validate().is_err(), "zero RPS should be rejected");
    }

    #[test]
    fn test_load_test_config_validate_rejects_zero_duration() {
        let config = LoadTestConfig {
            api_url: "http://localhost:8402".to_string(),
            rps: 10,
            duration_secs: 0,
            concurrency: 10,
            mode: LoadTestMode::DevBypass,
            tier_weights: TierWeights::default(),
            slo: SloThresholds::default(),
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };
        assert!(config.validate().is_err());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture 2>&1 | head -30`
Expected: Compilation error — module doesn't exist yet.

- [ ] **Step 3: Create config.rs with types and validation**

In `crates/cli/src/commands/loadtest/config.rs`:

```rust
use anyhow::{anyhow, Context, Result};

/// Payment mode for the load test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadTestMode {
    /// No payment — fastest iteration, validates HTTP path only.
    DevBypass,
    /// Real SPL TransferChecked per request.
    Exact,
    /// Real Anchor escrow deposit per request.
    Escrow,
}

impl std::str::FromStr for LoadTestMode {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_lowercase().as_str() {
            "dev-bypass" | "devbypass" | "bypass" => Ok(Self::DevBypass),
            "exact" | "direct" => Ok(Self::Exact),
            "escrow" => Ok(Self::Escrow),
            other => Err(anyhow!("unknown load test mode: '{other}'. Use: dev-bypass, exact, escrow")),
        }
    }
}

/// Tier distribution weights (must sum to 100).
#[derive(Debug, Clone)]
pub struct TierWeights {
    pub simple: u8,
    pub medium: u8,
    pub complex: u8,
    pub reasoning: u8,
}

impl Default for TierWeights {
    fn default() -> Self {
        Self {
            simple: 40,
            medium: 30,
            complex: 20,
            reasoning: 10,
        }
    }
}

impl TierWeights {
    /// Parse from "simple=40,medium=30,complex=20,reasoning=10" format.
    pub fn parse(input: &str) -> Result<Self> {
        let mut weights = Self { simple: 0, medium: 0, complex: 0, reasoning: 0 };
        for pair in input.split(',') {
            let (key, val) = pair
                .split_once('=')
                .with_context(|| format!("invalid tier weight pair: '{pair}'"))?;
            let v: u8 = val.trim().parse()
                .with_context(|| format!("invalid weight value: '{val}'"))?;
            match key.trim() {
                "simple" => weights.simple = v,
                "medium" => weights.medium = v,
                "complex" => weights.complex = v,
                "reasoning" => weights.reasoning = v,
                other => return Err(anyhow!("unknown tier: '{other}'")),
            }
        }
        let sum = weights.simple as u16 + weights.medium as u16
            + weights.complex as u16 + weights.reasoning as u16;
        if sum != 100 {
            return Err(anyhow!("tier weights must sum to 100, got {sum}"));
        }
        Ok(weights)
    }
}

/// SLO thresholds for pass/fail determination.
#[derive(Debug, Clone)]
pub struct SloThresholds {
    /// Maximum acceptable p99 latency in milliseconds.
    pub p99_ms: u64,
    /// Maximum acceptable error rate (0.0 to 1.0).
    pub error_rate: f64,
}

impl Default for SloThresholds {
    fn default() -> Self {
        Self {
            p99_ms: 5000,
            error_rate: 0.05,
        }
    }
}

/// Validated load test configuration.
#[derive(Debug, Clone)]
pub struct LoadTestConfig {
    pub api_url: String,
    pub rps: u64,
    pub duration_secs: u64,
    pub concurrency: usize,
    pub mode: LoadTestMode,
    pub tier_weights: TierWeights,
    pub slo: SloThresholds,
    pub report_json: Option<String>,
    pub prometheus_url: Option<String>,
    pub dry_run: bool,
}

impl LoadTestConfig {
    /// Validate the configuration, returning an error for invalid combinations.
    pub fn validate(&self) -> Result<()> {
        if self.rps == 0 {
            return Err(anyhow!("--rps must be greater than zero"));
        }
        if self.duration_secs == 0 {
            return Err(anyhow!("--duration must be greater than zero"));
        }
        if self.concurrency == 0 {
            return Err(anyhow!("--concurrency must be greater than zero"));
        }
        if self.api_url.is_empty() {
            return Err(anyhow!("--api-url must not be empty"));
        }
        // Warn when payment modes use unencrypted HTTP (dev/local is legitimate).
        if matches!(self.mode, LoadTestMode::Exact | LoadTestMode::Escrow)
            && self.api_url.starts_with("http://")
        {
            eprintln!(
                "WARNING: Signed Solana transactions will be sent over unencrypted HTTP. \
                 Use --api-url https://... for production."
            );
        }
        Ok(())
    }

    /// Total number of requests this config will dispatch.
    pub fn total_requests(&self) -> u64 {
        self.rps * self.duration_secs
    }
}
```

- [ ] **Step 4: Create mod.rs with clap args and entry point stub**

In `crates/cli/src/commands/loadtest/mod.rs`:

```rust
pub mod config;

use anyhow::Result;
use clap::Args;

use self::config::{LoadTestConfig, LoadTestMode, TierWeights, SloThresholds};

/// Load test the RustyClawRouter gateway with real payment transactions.
#[derive(Args, Debug)]
pub struct LoadTestArgs {
    /// Requests per second (constant arrival rate).
    #[arg(long, default_value = "10")]
    pub rps: u64,

    /// Test duration (e.g., "60s", "5m"). Parsed as seconds.
    #[arg(long, default_value = "30s")]
    pub duration: String,

    /// Maximum concurrent in-flight requests.
    #[arg(long, default_value = "20")]
    pub concurrency: usize,

    /// Payment mode: dev-bypass, exact, escrow.
    #[arg(long, default_value = "dev-bypass")]
    pub mode: String,

    /// Tier weight distribution: "simple=40,medium=30,complex=20,reasoning=10".
    #[arg(long)]
    pub tier_weights: Option<String>,

    /// P99 latency SLO in milliseconds. Test fails if exceeded.
    #[arg(long, default_value = "5000")]
    pub slo_p99_ms: u64,

    /// Error rate SLO (0.0 to 1.0). Test fails if exceeded.
    #[arg(long, default_value = "0.05")]
    pub slo_error_rate: f64,

    /// Path to write JSON report file.
    #[arg(long)]
    pub report_json: Option<String>,

    /// Prometheus endpoint URL for SLO validation.
    #[arg(long)]
    pub prometheus_url: Option<String>,

    /// Print what would happen without sending requests.
    #[arg(long)]
    pub dry_run: bool,
}

/// Parse a duration string like "60s", "5m", "1h" into seconds.
fn parse_duration_secs(input: &str) -> Result<u64> {
    let input = input.trim();
    if let Some(s) = input.strip_suffix('s') {
        return s.parse::<u64>().map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"));
    }
    if let Some(m) = input.strip_suffix('m') {
        let mins: u64 = m.parse().map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"))?;
        return Ok(mins * 60);
    }
    if let Some(h) = input.strip_suffix('h') {
        let hours: u64 = h.parse().map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"))?;
        return Ok(hours * 3600);
    }
    // Bare number = seconds.
    input.parse::<u64>().map_err(|_| anyhow::anyhow!("invalid duration: '{input}'. Use e.g. 60s, 5m, 1h"))
}

impl LoadTestArgs {
    /// Convert parsed CLI args into a validated config.
    pub fn into_config(self, api_url: &str) -> Result<LoadTestConfig> {
        let mode: LoadTestMode = self.mode.parse()?;
        let tier_weights = match self.tier_weights.as_deref() {
            Some(tw) => TierWeights::parse(tw)?,
            None => TierWeights::default(),
        };
        let duration_secs = parse_duration_secs(&self.duration)?;
        let config = LoadTestConfig {
            api_url: api_url.to_string(),
            rps: self.rps,
            duration_secs,
            concurrency: self.concurrency,
            mode,
            tier_weights,
            slo: SloThresholds {
                p99_ms: self.slo_p99_ms,
                error_rate: self.slo_error_rate,
            },
            report_json: self.report_json,
            prometheus_url: self.prometheus_url,
            dry_run: self.dry_run,
        };
        config.validate()?;
        Ok(config)
    }
}

/// Entry point for `rcr loadtest`.
pub async fn run(api_url: &str, args: LoadTestArgs) -> Result<()> {
    let config = args.into_config(api_url)?;

    if config.dry_run {
        println!("=== DRY RUN ===");
        println!("Target:       {}", config.api_url);
        println!("Mode:         {:?}", config.mode);
        println!("RPS:          {}", config.rps);
        println!("Duration:     {}s", config.duration_secs);
        println!("Concurrency:  {}", config.concurrency);
        println!("Total reqs:   {}", config.total_requests());
        println!("Tier weights: simple={}, medium={}, complex={}, reasoning={}",
            config.tier_weights.simple,
            config.tier_weights.medium,
            config.tier_weights.complex,
            config.tier_weights.reasoning,
        );
        println!("SLO p99:      {}ms", config.slo.p99_ms);
        println!("SLO err rate: {:.2}%", config.slo.error_rate * 100.0);
        if let Some(ref path) = config.report_json {
            println!("JSON report:  {path}");
        }
        if let Some(ref url) = config.prometheus_url {
            println!("Prometheus:   {url}");
        }
        println!("=== No requests sent ===");
        return Ok(());
    }

    // TODO: dispatcher + workers + report (Tasks 2-8)
    println!("Load test not yet fully implemented. Use --dry-run to preview config.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration_secs("60s").unwrap(), 60);
        assert_eq!(parse_duration_secs("120s").unwrap(), 120);
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration_secs("5m").unwrap(), 300);
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration_secs("1h").unwrap(), 3600);
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration_secs("30").unwrap(), 30);
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration_secs("abc").is_err());
        assert!(parse_duration_secs("").is_err());
    }
}
```

- [ ] **Step 5: Wire into CLI — modify mod.rs, main.rs, Cargo.toml**

In `crates/cli/src/commands/mod.rs`, add:
```rust
pub mod loadtest;
```

In `crates/cli/src/main.rs`, add `Loadtest` variant to the `Commands` enum:
```rust
/// Load test the gateway with configurable concurrency and payment modes
Loadtest(commands::loadtest::LoadTestArgs),
```

Add the match arm:
```rust
Commands::Loadtest(args) => commands::loadtest::run(&cli.api_url, args).await?,
```

In `crates/cli/Cargo.toml`, add under `[dependencies]`:
```toml
hdrhistogram = "7"
secrecy = "0.8"
```

- [ ] **Step 6: Run tests to verify everything compiles and passes**

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture`
Expected: All config + duration parsing tests pass.

Run: `cargo check -p rustyclawrouter-cli`
Expected: Clean compilation with no errors.

- [ ] **Step 7: Commit**

```bash
git add crates/cli/src/commands/loadtest/ crates/cli/src/commands/mod.rs crates/cli/src/main.rs crates/cli/Cargo.toml
git commit -m "feat: scaffold rcr loadtest subcommand with config parsing and dry-run mode"
```

---

## Task 2: Metrics Collector (AtomicU64 + HdrHistogram)

**Files:**
- Create: `crates/cli/src/commands/loadtest/metrics.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod metrics;`)

Thread-safe metrics collector using atomic counters for request outcomes and HdrHistogram for latency percentile tracking. Must be `Send + Sync` for sharing across tokio tasks.

- [ ] **Step 1: Write the failing tests**

In `crates/cli/src/commands/loadtest/metrics.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn test_new_collector_starts_at_zero() {
        let m = MetricsCollector::new();
        let snap = m.snapshot();
        assert_eq!(snap.total_requests, 0);
        assert_eq!(snap.successful, 0);
        assert_eq!(snap.payment_required_402, 0);
        assert_eq!(snap.rate_limited_429, 0);
        assert_eq!(snap.server_errors_5xx, 0);
        assert_eq!(snap.timeouts, 0);
        assert_eq!(snap.other_errors, 0);
        assert_eq!(snap.dropped_requests, 0);
    }

    #[test]
    fn test_record_dropped_increments() {
        let m = MetricsCollector::new();
        m.record_dropped();
        m.record_dropped();
        let snap = m.snapshot();
        assert_eq!(snap.dropped_requests, 2);
        // Dropped requests do not count toward total_requests.
        assert_eq!(snap.total_requests, 0);
    }

    #[test]
    fn test_record_success_increments() {
        let m = MetricsCollector::new();
        m.record_success(Duration::from_millis(150));
        m.record_success(Duration::from_millis(200));
        let snap = m.snapshot();
        assert_eq!(snap.total_requests, 2);
        assert_eq!(snap.successful, 2);
    }

    #[test]
    fn test_record_various_errors() {
        let m = MetricsCollector::new();
        m.record_outcome(RequestOutcome::PaymentRequired402, Duration::from_millis(10));
        m.record_outcome(RequestOutcome::RateLimited429, Duration::from_millis(20));
        m.record_outcome(RequestOutcome::ServerError5xx, Duration::from_millis(30));
        m.record_outcome(RequestOutcome::Timeout, Duration::from_millis(40));
        m.record_outcome(RequestOutcome::OtherError, Duration::from_millis(50));
        let snap = m.snapshot();
        assert_eq!(snap.total_requests, 5);
        assert_eq!(snap.payment_required_402, 1);
        assert_eq!(snap.rate_limited_429, 1);
        assert_eq!(snap.server_errors_5xx, 1);
        assert_eq!(snap.timeouts, 1);
        assert_eq!(snap.other_errors, 1);
    }

    #[test]
    fn test_latency_percentiles() {
        let m = MetricsCollector::new();
        // Record 100 requests with latencies 1ms through 100ms.
        for i in 1..=100 {
            m.record_success(Duration::from_millis(i));
        }
        let snap = m.snapshot();
        // p50 should be near 50ms, p99 near 99-100ms.
        assert!(snap.p50_ms >= 45 && snap.p50_ms <= 55, "p50 was {}ms", snap.p50_ms);
        assert!(snap.p95_ms >= 90 && snap.p95_ms <= 100, "p95 was {}ms", snap.p95_ms);
        assert!(snap.p99_ms >= 95 && snap.p99_ms <= 100, "p99 was {}ms", snap.p99_ms);
    }

    #[test]
    fn test_error_rate_calculation() {
        let m = MetricsCollector::new();
        for _ in 0..90 {
            m.record_success(Duration::from_millis(10));
        }
        for _ in 0..10 {
            m.record_outcome(RequestOutcome::ServerError5xx, Duration::from_millis(10));
        }
        let snap = m.snapshot();
        let rate = snap.error_rate();
        assert!((rate - 0.10).abs() < 0.01, "error rate should be ~10%, got {rate}");
    }

    #[test]
    fn test_snapshot_empty_percentiles() {
        let m = MetricsCollector::new();
        let snap = m.snapshot();
        // No data recorded — percentiles should be 0.
        assert_eq!(snap.p50_ms, 0);
        assert_eq!(snap.p95_ms, 0);
        assert_eq!(snap.p99_ms, 0);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli metrics -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `MetricsCollector` not defined.

- [ ] **Step 3: Implement MetricsCollector**

In `crates/cli/src/commands/loadtest/metrics.rs`:

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use hdrhistogram::Histogram;

/// Categorized request outcome for error bucketing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestOutcome {
    Success,
    PaymentRequired402,
    RateLimited429,
    ServerError5xx,
    Timeout,
    OtherError,
}

/// Thread-safe metrics collector for load test results.
///
/// Uses atomic counters for throughput and `HdrHistogram` (behind a Mutex)
/// for latency percentile tracking. Safe to share via `Arc<MetricsCollector>`
/// across tokio tasks.
pub struct MetricsCollector {
    total_requests: AtomicU64,
    successful: AtomicU64,
    payment_required_402: AtomicU64,
    rate_limited_429: AtomicU64,
    server_errors_5xx: AtomicU64,
    timeouts: AtomicU64,
    other_errors: AtomicU64,
    dropped_requests: AtomicU64,
    latency_hist: Mutex<Histogram<u64>>,
}

impl MetricsCollector {
    /// Create a new collector. Histogram tracks latencies up to 60 seconds
    /// with 3 significant digits of precision.
    pub fn new() -> Self {
        Self {
            total_requests: AtomicU64::new(0),
            successful: AtomicU64::new(0),
            payment_required_402: AtomicU64::new(0),
            rate_limited_429: AtomicU64::new(0),
            server_errors_5xx: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            other_errors: AtomicU64::new(0),
            dropped_requests: AtomicU64::new(0),
            // Track up to 60_000ms with 3 significant figures.
            latency_hist: Mutex::new(
                Histogram::new_with_bounds(1, 60_000, 3)
                    .expect("histogram bounds are valid"),
            ),
        }
    }

    /// Record a successful request with its latency.
    pub fn record_success(&self, latency: Duration) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.successful.fetch_add(1, Ordering::Relaxed);
        self.record_latency(latency);
    }

    /// Record a request outcome (success or error category) with its latency.
    pub fn record_outcome(&self, outcome: RequestOutcome, latency: Duration) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        match outcome {
            RequestOutcome::Success => self.successful.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::PaymentRequired402 => self.payment_required_402.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::RateLimited429 => self.rate_limited_429.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::ServerError5xx => self.server_errors_5xx.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::Timeout => self.timeouts.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::OtherError => self.other_errors.fetch_add(1, Ordering::Relaxed),
        };
        self.record_latency(latency);
    }

    /// Record a request that was dropped because the semaphore was full.
    pub fn record_dropped(&self) {
        self.dropped_requests.fetch_add(1, Ordering::Relaxed);
    }

    fn record_latency(&self, latency: Duration) {
        let ms = latency.as_millis().min(60_000) as u64;
        let ms = ms.max(1); // Histogram minimum is 1.
        // Mutex poisoning is non-recoverable in a load test — unwrap is acceptable.
        let mut hist = self.latency_hist.lock().expect("histogram lock poisoned");
        // Saturate at max trackable value rather than error.
        let _ = hist.record(ms);
    }

    /// Take a point-in-time snapshot of all metrics.
    pub fn snapshot(&self) -> MetricsSnapshot {
        let hist = self.latency_hist.lock().expect("histogram lock poisoned");
        MetricsSnapshot {
            total_requests: self.total_requests.load(Ordering::Relaxed),
            successful: self.successful.load(Ordering::Relaxed),
            payment_required_402: self.payment_required_402.load(Ordering::Relaxed),
            rate_limited_429: self.rate_limited_429.load(Ordering::Relaxed),
            server_errors_5xx: self.server_errors_5xx.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            other_errors: self.other_errors.load(Ordering::Relaxed),
            dropped_requests: self.dropped_requests.load(Ordering::Relaxed),
            p50_ms: hist.value_at_quantile(0.50),
            p95_ms: hist.value_at_quantile(0.95),
            p99_ms: hist.value_at_quantile(0.99),
            min_ms: if hist.len() > 0 { hist.min() } else { 0 },
            max_ms: if hist.len() > 0 { hist.max() } else { 0 },
            mean_ms: if hist.len() > 0 { hist.mean() as u64 } else { 0 },
        }
    }
}

/// Immutable point-in-time snapshot of load test metrics.
#[derive(Debug, Clone, serde::Serialize)]
pub struct MetricsSnapshot {
    pub total_requests: u64,
    pub successful: u64,
    pub payment_required_402: u64,
    pub rate_limited_429: u64,
    pub server_errors_5xx: u64,
    pub timeouts: u64,
    pub other_errors: u64,
    pub dropped_requests: u64,
    pub p50_ms: u64,
    pub p95_ms: u64,
    pub p99_ms: u64,
    pub min_ms: u64,
    pub max_ms: u64,
    pub mean_ms: u64,
}

impl MetricsSnapshot {
    /// Compute the error rate as a fraction (0.0 to 1.0).
    /// Errors = everything except successful requests.
    pub fn error_rate(&self) -> f64 {
        if self.total_requests == 0 {
            return 0.0;
        }
        let errors = self.total_requests - self.successful;
        errors as f64 / self.total_requests as f64
    }

    /// Effective requests per second (total / wall-clock duration).
    pub fn effective_rps(&self, duration_secs: u64) -> f64 {
        if duration_secs == 0 {
            return 0.0;
        }
        self.total_requests as f64 / duration_secs as f64
    }
}
```

- [ ] **Step 4: Add module declaration**

In `crates/cli/src/commands/loadtest/mod.rs`, add at the top:
```rust
pub mod metrics;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p rustyclawrouter-cli metrics -- --nocapture`
Expected: All 7 metrics tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/metrics.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add thread-safe MetricsCollector with HdrHistogram latency tracking"
```

---

## Task 3: Payment Strategy Trait and DevBypass Implementation

**Files:**
- Create: `crates/cli/src/commands/loadtest/payment.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod payment;`)

Define the `PaymentStrategy` async trait with three implementations. This task implements only `DevBypass` (the others come in Tasks 6-7). The trait abstracts payment signing so the worker doesn't care which mode is active.

- [ ] **Step 1: Write failing tests**

In `crates/cli/src/commands/loadtest/payment.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_dev_bypass_returns_none() {
        let strategy = DevBypassStrategy;
        let result = strategy
            .prepare_payment("http://localhost:8402", &serde_json::json!({}), &[])
            .await
            .expect("dev bypass should not error");
        assert!(result.is_none(), "dev bypass should return no payment header");
    }

    #[tokio::test]
    async fn test_dev_bypass_display_name() {
        let strategy = DevBypassStrategy;
        assert_eq!(strategy.name(), "dev-bypass");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli payment -- --nocapture 2>&1 | head -20`
Expected: Compilation error — types not defined.

- [ ] **Step 3: Implement PaymentStrategy trait and DevBypassStrategy**

In `crates/cli/src/commands/loadtest/payment.rs`:

```rust
use anyhow::Result;

use x402::types::PaymentAccept;

/// Trait abstracting payment signing for load test workers.
///
/// Each mode (dev-bypass, exact, escrow) implements this trait.
/// The worker calls `prepare_payment` after receiving a 402 response
/// and uses the returned header value (if any) to retry the request.
#[async_trait::async_trait]
pub trait PaymentStrategy: Send + Sync {
    /// Human-readable name for reporting.
    fn name(&self) -> &'static str;

    /// Prepare the PAYMENT-SIGNATURE header value for a request.
    ///
    /// Returns `Ok(None)` if no payment is needed (dev-bypass mode).
    /// Returns `Ok(Some(header_value))` with the base64-encoded payment payload.
    /// The `accepts` slice comes from the 402 response's PaymentRequired.accepts.
    async fn prepare_payment(
        &self,
        rpc_url: &str,
        request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>>;
}

/// No-op payment strategy for dev-bypass mode.
///
/// Relies on the gateway having `RCR_DEV_BYPASS_PAYMENT=true` set.
/// No wallet needed, no Solana RPC calls.
pub struct DevBypassStrategy;

#[async_trait::async_trait]
impl PaymentStrategy for DevBypassStrategy {
    fn name(&self) -> &'static str {
        "dev-bypass"
    }

    async fn prepare_payment(
        &self,
        _rpc_url: &str,
        _request_body: &serde_json::Value,
        _accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        Ok(None)
    }
}
```

- [ ] **Step 4: Add module declaration and async-trait dependency**

In `crates/cli/src/commands/loadtest/mod.rs`, add:
```rust
pub mod payment;
```

In `crates/cli/Cargo.toml`, add under `[dependencies]` (if not already present):
```toml
async-trait = { workspace = true }
```

Check if `async-trait` is already a workspace dependency — it is (line 20 of workspace Cargo.toml). If already in cli Cargo.toml, skip.

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli payment -- --nocapture`
Expected: Both payment tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/payment.rs crates/cli/src/commands/loadtest/mod.rs crates/cli/Cargo.toml
git commit -m "feat: add PaymentStrategy trait with DevBypass implementation"
```

---

## Task 4: Request Worker (Full 402 Dance)

**Files:**
- Create: `crates/cli/src/commands/loadtest/worker.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod worker;`)

The worker performs a single request lifecycle: send POST -> if 402, call PaymentStrategy -> resubmit with header -> record outcome to MetricsCollector. This is the core of the load test.

- [ ] **Step 1: Write failing tests**

In `crates/cli/src/commands/loadtest/worker.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::metrics::MetricsCollector;
    use crate::commands::loadtest::payment::DevBypassStrategy;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_body() -> serde_json::Value {
        serde_json::json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "test prompt"}]
        })
    }

    #[tokio::test]
    async fn test_worker_success_on_200() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "hello"}}]
            })))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let outcome = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            std::time::Instant::now(),
        )
        .await;

        assert!(outcome.is_ok());
        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.successful, 1);
    }

    #[tokio::test]
    async fn test_worker_records_5xx() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            std::time::Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.server_errors_5xx, 1);
    }

    #[tokio::test]
    async fn test_worker_records_429() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            std::time::Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.rate_limited_429, 1);
    }

    #[tokio::test]
    async fn test_worker_connection_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dead_url = format!("http://127.0.0.1:{port}");

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &dead_url,
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            std::time::Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.other_errors, 1);
    }

    #[tokio::test]
    async fn test_worker_402_dance_with_payment() {
        // Mock server returns 402 on first call (no PAYMENT-SIGNATURE header),
        // then 200 on second call (when header is present).
        use wiremock::matchers::header_exists;

        let mock = MockServer::start().await;

        // First call: no payment header → 402 with PaymentRequired body.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(wiremock::matchers::header_exists("PAYMENT-SIGNATURE").not())
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": "{\"accepts\":[{\"scheme\":\"exact\",\"network\":\"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp\",\"amount\":\"1000\",\"asset\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\",\"pay_to\":\"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM\",\"max_timeout_seconds\":300}]}"
                }
            })))
            .expect(1)
            .mount(&mock)
            .await;

        // Second call: has PAYMENT-SIGNATURE header → 200 success.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "paid response"}}]
            })))
            .expect(1)
            .mount(&mock)
            .await;

        // Build a minimal ExactPaymentStrategy with a mock RPC for blockhash.
        let rpc_mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "value": {
                        "blockhash": "11111111111111111111111111111111",
                        "lastValidBlockHeight": 9999
                    }
                }
            })))
            .mount(&rpc_mock)
            .await;

        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        let keypair_b58 = bs58::encode(&full).into_string();

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(
            super::payment::ExactPaymentStrategy::new(keypair_b58, reqwest::Client::new())
        );

        let outcome = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            &rpc_mock.uri(),
            &metrics,
            std::time::Instant::now(),
        )
        .await;

        assert!(outcome.is_ok(), "402-dance should succeed: {:?}", outcome.err());
        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.successful, 1, "request should be recorded as successful after 402 dance");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli worker -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `execute_request` not defined.

- [ ] **Step 3: Implement the request worker**

In `crates/cli/src/commands/loadtest/worker.rs`:

```rust
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use x402::types::PaymentRequired;

use super::metrics::{MetricsCollector, RequestOutcome};
use super::payment::PaymentStrategy;

/// Execute a single load test request: the full 402-dance lifecycle.
///
/// 1. POST to /v1/chat/completions
/// 2. If 200 → record success
/// 3. If 402 → parse PaymentRequired, call strategy.prepare_payment, retry with header
/// 4. If 429/5xx/other → record error category
///
/// Latency is measured from `scheduled_at` (the dispatcher's tick instant),
/// NOT from when this function starts. This includes any queuing delay and
/// prevents coordinated omission bias in the reported percentiles.
pub async fn execute_request(
    client: &reqwest::Client,
    api_url: &str,
    body: &serde_json::Value,
    strategy: &dyn PaymentStrategy,
    rpc_url: &str,
    metrics: &Arc<MetricsCollector>,
    scheduled_at: Instant,
) -> Result<()> {
    let endpoint = format!("{api_url}/v1/chat/completions");
    let start = scheduled_at;

    // First request (may return 402).
    let resp = match client.post(&endpoint).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            let latency = start.elapsed();
            if e.is_timeout() {
                metrics.record_outcome(RequestOutcome::Timeout, latency);
            } else {
                metrics.record_outcome(RequestOutcome::OtherError, latency);
            }
            return Err(e.into());
        }
    };

    let status = resp.status().as_u16();

    match status {
        200..=299 => {
            // Success without payment (dev-bypass mode or free model).
            let latency = start.elapsed();
            metrics.record_success(latency);
            return Ok(());
        }
        402 => {
            // Payment required — execute the 402 dance.
            let error_body: serde_json::Value = resp
                .json()
                .await
                .context("failed to parse 402 response body")?;

            let error_msg = error_body["error"]["message"]
                .as_str()
                .unwrap_or("");

            let payment_required: PaymentRequired = serde_json::from_str(error_msg)
                .context("failed to parse PaymentRequired from 402")?;

            let payment_header = strategy
                .prepare_payment(rpc_url, body, &payment_required.accepts)
                .await
                .context("payment strategy failed")?;

            match payment_header {
                Some(header_value) => {
                    // Retry with payment.
                    let retry_resp = match client
                        .post(&endpoint)
                        .header("PAYMENT-SIGNATURE", &header_value)
                        .json(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            let latency = start.elapsed();
                            if e.is_timeout() {
                                metrics.record_outcome(RequestOutcome::Timeout, latency);
                            } else {
                                metrics.record_outcome(RequestOutcome::OtherError, latency);
                            }
                            return Err(e.into());
                        }
                    };

                    let retry_status = retry_resp.status().as_u16();
                    let latency = start.elapsed();

                    match retry_status {
                        200..=299 => metrics.record_success(latency),
                        429 => metrics.record_outcome(RequestOutcome::RateLimited429, latency),
                        500..=599 => metrics.record_outcome(RequestOutcome::ServerError5xx, latency),
                        _ => metrics.record_outcome(RequestOutcome::OtherError, latency),
                    }
                }
                None => {
                    // DevBypass mode: 402 means the gateway isn't in bypass mode.
                    // Record as 402 error.
                    let latency = start.elapsed();
                    metrics.record_outcome(RequestOutcome::PaymentRequired402, latency);
                }
            }
        }
        429 => {
            let latency = start.elapsed();
            metrics.record_outcome(RequestOutcome::RateLimited429, latency);
        }
        500..=599 => {
            let latency = start.elapsed();
            metrics.record_outcome(RequestOutcome::ServerError5xx, latency);
        }
        _ => {
            let latency = start.elapsed();
            metrics.record_outcome(RequestOutcome::OtherError, latency);
        }
    }

    Ok(())
}
```

- [ ] **Step 4: Add module declaration**

In `crates/cli/src/commands/loadtest/mod.rs`, add:
```rust
pub mod worker;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli worker -- --nocapture`
Expected: All 4 worker tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/worker.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add request worker with full 402-dance lifecycle and error categorization"
```

---

## Task 5: Constant-Arrival-Rate Dispatcher

**Files:**
- Create: `crates/cli/src/commands/loadtest/dispatcher.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod dispatcher;`, wire into `run()`)

The dispatcher sends exactly N requests per second using `tokio::time::interval`, with a `Semaphore` capping concurrent in-flight requests. This prevents coordinated omission bias — if responses are slow, the dispatcher still sends at the configured rate (until the semaphore fills).

- [ ] **Step 1: Write failing tests**

In `crates/cli/src/commands/loadtest/dispatcher.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::metrics::MetricsCollector;
    use crate::commands::loadtest::payment::DevBypassStrategy;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_dispatcher_sends_correct_count() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&mock)
            .await;

        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let config = DispatcherConfig {
            api_url: mock.uri(),
            rpc_url: String::new(),
            rps: 10,
            duration_secs: 1,
            concurrency: 20,
        };

        run_dispatcher(config, strategy, metrics.clone()).await;

        let snap = metrics.snapshot();
        // Should dispatch exactly rps * duration = 10 requests.
        // With 20 concurrency slots and fast responses, none should be dropped.
        assert_eq!(snap.total_requests, 10, "expected 10 requests, got {}", snap.total_requests);
        assert_eq!(snap.successful, 10);
        assert_eq!(snap.dropped_requests, 0, "no requests should be dropped with excess concurrency");
    }

    #[tokio::test]
    async fn test_dispatcher_respects_concurrency_limit() {
        // Slow server: each response takes 200ms.
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({
                        "choices": [{"message": {"content": "ok"}}]
                    }))
                    .set_delay(std::time::Duration::from_millis(200)),
            )
            .mount(&mock)
            .await;

        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        // 50 RPS but only 5 concurrent slots with 200ms latency.
        // With try_acquire_owned, requests beyond the 5 in-flight slots
        // are dropped rather than queued.
        let config = DispatcherConfig {
            api_url: mock.uri(),
            rpc_url: String::new(),
            rps: 50,
            duration_secs: 1,
            concurrency: 5,
        };

        run_dispatcher(config, strategy, metrics.clone()).await;

        let snap = metrics.snapshot();
        // Some requests should complete successfully.
        assert!(snap.total_requests > 0, "should have dispatched some requests");
        // With 200ms latency and only 5 slots, many of the 50 requests
        // should be dropped because try_acquire_owned fails.
        assert!(snap.dropped_requests > 0,
            "expected some dropped requests under semaphore saturation, got 0");
        // Total completed + dropped should account for all 50 attempts.
        assert_eq!(snap.total_requests + snap.dropped_requests, 50,
            "completed ({}) + dropped ({}) should equal total attempts (50)",
            snap.total_requests, snap.dropped_requests);
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli dispatcher -- --nocapture 2>&1 | head -20`
Expected: Compilation error.

- [ ] **Step 3: Implement the dispatcher**

In `crates/cli/src/commands/loadtest/dispatcher.rs`:

```rust
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Semaphore;
use tokio::time;

use super::metrics::MetricsCollector;
use super::payment::PaymentStrategy;
use super::worker;

/// Configuration for the dispatcher (subset of LoadTestConfig).
pub struct DispatcherConfig {
    pub api_url: String,
    pub rpc_url: String,
    pub rps: u64,
    pub duration_secs: u64,
    pub concurrency: usize,
}

/// Generate a request body for a given tier.
fn build_request_body(tier: &str) -> serde_json::Value {
    let (model, prompt) = match tier {
        "simple" => ("auto", "What is 2+2?"),
        "medium" => ("auto", "Explain how Solana's proof of history works in detail."),
        "complex" => ("auto", "Write a Rust function that implements a concurrent LRU cache with TTL support using tokio. Include error handling and tests."),
        "reasoning" => ("auto", "Given a distributed system with 5 nodes using Raft consensus, prove that the system maintains linearizability when network partitions occur and heal within the election timeout. Show your reasoning step by step."),
        _ => ("auto", "Hello"),
    };
    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}]
    })
}

/// Select a tier based on weighted random distribution.
fn select_tier(weights: &super::config::TierWeights) -> &'static str {
    let r: u8 = (rand_u8() % 100) + 1; // 1-100 inclusive
    if r <= weights.simple {
        "simple"
    } else if r <= weights.simple + weights.medium {
        "medium"
    } else if r <= weights.simple + weights.medium + weights.complex {
        "complex"
    } else {
        "reasoning"
    }
}

/// Simple pseudo-random u8 using getrandom.
fn rand_u8() -> u8 {
    let mut buf = [0u8; 1];
    getrandom::getrandom(&mut buf).unwrap_or_default();
    buf[0]
}

/// Run the constant-arrival-rate dispatcher.
///
/// Sends exactly `rps` requests per second for `duration_secs` seconds.
/// Uses `try_acquire_owned` on a semaphore to cap concurrent in-flight
/// requests at `concurrency`. When the semaphore is full, the request is
/// dropped (counted in `MetricsCollector::dropped_requests`) rather than
/// blocking the dispatch loop — this prevents coordinated omission bias.
///
/// Latency is measured from the `interval.tick()` instant (scheduled time),
/// NOT from when the worker starts, so queuing delay is included in the
/// reported percentiles.
///
/// Uses `JoinSet` instead of `Vec<JoinHandle>` for incremental task reaping.
pub async fn run_dispatcher(
    config: DispatcherConfig,
    strategy: Arc<dyn PaymentStrategy>,
    metrics: Arc<MetricsCollector>,
) {
    let total_requests = config.rps * config.duration_secs;
    let interval_duration = Duration::from_secs_f64(1.0 / config.rps as f64);
    let semaphore = Arc::new(Semaphore::new(config.concurrency));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("failed to build HTTP client");

    let api_url = Arc::new(config.api_url);
    let rpc_url = Arc::new(config.rpc_url);

    let mut interval = time::interval(interval_duration);
    // Skip missed ticks instead of bursting — prevents thundering herd
    // after semaphore saturation.
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    let mut join_set = tokio::task::JoinSet::new();

    // Default tier weights for now — will be threaded from config in the wiring step.
    let tier_weights = super::config::TierWeights::default();

    for _i in 0..total_requests {
        interval.tick().await;

        // Record scheduled time BEFORE attempting semaphore acquire.
        // This is the latency start — includes any queuing delay.
        let scheduled_at = std::time::Instant::now();

        // Non-blocking acquire: if all permits are taken, drop the request.
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                metrics.record_dropped();
                continue;
            }
        };

        let client = client.clone();
        let api_url = api_url.clone();
        let rpc_url = rpc_url.clone();
        let strategy = strategy.clone();
        let metrics = metrics.clone();

        let tier = select_tier(&tier_weights);
        let body = build_request_body(tier);

        join_set.spawn(async move {
            let _ = worker::execute_request(
                &client,
                &api_url,
                &body,
                strategy.as_ref(),
                &rpc_url,
                &metrics,
                scheduled_at,
            )
            .await;
            drop(permit); // Release semaphore slot.
        });
    }

    // Wait for all in-flight requests to complete.
    while join_set.join_next().await.is_some() {}
}
```

- [ ] **Step 4: Add module declaration**

In `crates/cli/src/commands/loadtest/mod.rs`, add:
```rust
pub mod dispatcher;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli dispatcher -- --nocapture`
Expected: Both dispatcher tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/dispatcher.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add constant-arrival-rate dispatcher with semaphore concurrency limiting"
```

---

## Task 6: Report Formatters (Terminal + JSON)

**Files:**
- Create: `crates/cli/src/commands/loadtest/report.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod report;`)

Terminal report with colored pass/fail and a JSON report for CI/CD integration. Includes SLO validation (p99 and error rate thresholds).

- [ ] **Step 1: Write failing tests**

In `crates/cli/src/commands/loadtest/report.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::config::SloThresholds;
    use crate::commands::loadtest::metrics::MetricsSnapshot;

    fn sample_snapshot() -> MetricsSnapshot {
        MetricsSnapshot {
            total_requests: 100,
            successful: 95,
            payment_required_402: 0,
            rate_limited_429: 2,
            server_errors_5xx: 2,
            timeouts: 1,
            other_errors: 0,
            dropped_requests: 0,
            p50_ms: 120,
            p95_ms: 450,
            p99_ms: 980,
            min_ms: 15,
            max_ms: 1200,
            mean_ms: 200,
        }
    }

    #[test]
    fn test_slo_check_passes() {
        let snap = sample_snapshot();
        let slo = SloThresholds { p99_ms: 2000, error_rate: 0.10 };
        let result = check_slo(&snap, &slo);
        assert!(result.passed, "SLO should pass: p99={}ms < 2000ms, err_rate={:.2}", snap.p99_ms, snap.error_rate());
    }

    #[test]
    fn test_slo_check_fails_p99() {
        let snap = sample_snapshot();
        let slo = SloThresholds { p99_ms: 500, error_rate: 0.10 };
        let result = check_slo(&snap, &slo);
        assert!(!result.passed, "SLO should fail: p99={}ms > 500ms", snap.p99_ms);
        assert!(result.violations.iter().any(|v| v.contains("p99")));
    }

    #[test]
    fn test_slo_check_fails_error_rate() {
        let snap = sample_snapshot();
        let slo = SloThresholds { p99_ms: 5000, error_rate: 0.01 };
        let result = check_slo(&snap, &slo);
        assert!(!result.passed, "SLO should fail: error_rate={:.2} > 0.01", snap.error_rate());
        assert!(result.violations.iter().any(|v| v.contains("error rate")));
    }

    #[test]
    fn test_json_report_serializes() {
        let snap = sample_snapshot();
        let slo = SloThresholds { p99_ms: 5000, error_rate: 0.10 };
        let report = build_json_report(&snap, &slo, 60, "dev-bypass");
        let json_str = serde_json::to_string_pretty(&report).expect("serialize");
        assert!(json_str.contains("total_requests"));
        assert!(json_str.contains("slo_passed"));
    }

    #[test]
    fn test_terminal_report_does_not_panic() {
        let snap = sample_snapshot();
        let slo = SloThresholds { p99_ms: 5000, error_rate: 0.10 };
        // Just verify it doesn't panic — output goes to stdout.
        print_terminal_report(&snap, &slo, 60, "dev-bypass");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli report -- --nocapture 2>&1 | head -20`
Expected: Compilation error.

- [ ] **Step 3: Implement report formatters**

In `crates/cli/src/commands/loadtest/report.rs`:

```rust
use crate::commands::loadtest::config::SloThresholds;
use crate::commands::loadtest::metrics::MetricsSnapshot;

/// Result of SLO validation.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SloResult {
    pub passed: bool,
    pub violations: Vec<String>,
}

/// Check metrics against SLO thresholds.
pub fn check_slo(snapshot: &MetricsSnapshot, slo: &SloThresholds) -> SloResult {
    let mut violations = Vec::new();

    if snapshot.p99_ms > slo.p99_ms {
        violations.push(format!(
            "p99 latency {}ms exceeds SLO threshold {}ms",
            snapshot.p99_ms, slo.p99_ms
        ));
    }

    let error_rate = snapshot.error_rate();
    if error_rate > slo.error_rate {
        violations.push(format!(
            "error rate {:.2}% exceeds SLO threshold {:.2}%",
            error_rate * 100.0,
            slo.error_rate * 100.0
        ));
    }

    SloResult {
        passed: violations.is_empty(),
        violations,
    }
}

/// JSON report structure for CI/CD integration.
#[derive(Debug, Clone, serde::Serialize)]
pub struct JsonReport {
    pub mode: String,
    pub duration_secs: u64,
    pub metrics: MetricsSnapshot,
    pub dropped_requests: u64,
    pub slo_passed: bool,
    pub slo_violations: Vec<String>,
    pub effective_rps: f64,
    pub error_rate_percent: f64,
    pub timestamp: String,
}

/// Build a JSON-serializable report.
pub fn build_json_report(
    snapshot: &MetricsSnapshot,
    slo: &SloThresholds,
    duration_secs: u64,
    mode: &str,
) -> JsonReport {
    let slo_result = check_slo(snapshot, slo);
    JsonReport {
        mode: mode.to_string(),
        duration_secs,
        dropped_requests: snapshot.dropped_requests,
        metrics: snapshot.clone(),
        slo_passed: slo_result.passed,
        slo_violations: slo_result.violations,
        effective_rps: snapshot.effective_rps(duration_secs),
        error_rate_percent: snapshot.error_rate() * 100.0,
        timestamp: chrono::Utc::now().to_rfc3339(),
    }
}

/// Print a formatted terminal report.
pub fn print_terminal_report(
    snapshot: &MetricsSnapshot,
    slo: &SloThresholds,
    duration_secs: u64,
    mode: &str,
) {
    let slo_result = check_slo(snapshot, slo);
    let effective_rps = snapshot.effective_rps(duration_secs);

    println!();
    println!("============================================================");
    println!("  RustyClawRouter Load Test Report");
    println!("============================================================");
    println!("  Mode:            {mode}");
    println!("  Duration:        {duration_secs}s");
    println!("  Effective RPS:   {effective_rps:.1}");
    println!();
    println!("  --- Results ---");
    println!("  Total requests:  {}", snapshot.total_requests);
    println!("  Successful:      {}", snapshot.successful);
    println!("  402 (unpaid):    {}", snapshot.payment_required_402);
    println!("  429 (rate lim):  {}", snapshot.rate_limited_429);
    println!("  5xx (server):    {}", snapshot.server_errors_5xx);
    println!("  Timeouts:        {}", snapshot.timeouts);
    println!("  Other errors:    {}", snapshot.other_errors);
    println!("  Dropped (full):  {}", snapshot.dropped_requests);
    println!("  Error rate:      {:.2}%", snapshot.error_rate() * 100.0);
    println!();
    println!("  --- Latency ---");
    println!("  Min:    {}ms", snapshot.min_ms);
    println!("  Mean:   {}ms", snapshot.mean_ms);
    println!("  p50:    {}ms", snapshot.p50_ms);
    println!("  p95:    {}ms", snapshot.p95_ms);
    println!("  p99:    {}ms", snapshot.p99_ms);
    println!("  Max:    {}ms", snapshot.max_ms);
    println!();
    println!("  --- SLO ---");
    println!("  p99 threshold:   {}ms (actual: {}ms) {}", slo.p99_ms, snapshot.p99_ms,
        if snapshot.p99_ms <= slo.p99_ms { "PASS" } else { "FAIL" });
    println!("  Error threshold: {:.2}% (actual: {:.2}%) {}", slo.error_rate * 100.0, snapshot.error_rate() * 100.0,
        if snapshot.error_rate() <= slo.error_rate { "PASS" } else { "FAIL" });
    println!();

    if slo_result.passed {
        println!("  Result: PASS");
    } else {
        println!("  Result: FAIL");
        for v in &slo_result.violations {
            println!("    - {v}");
        }
    }
    println!("============================================================");
}
```

- [ ] **Step 4: Add module declaration**

In `crates/cli/src/commands/loadtest/mod.rs`, add:
```rust
pub mod report;
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli report -- --nocapture`
Expected: All 5 report tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/report.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add terminal and JSON report formatters with SLO validation"
```

---

## Task 7: Wire Dispatcher + Report into run() Entry Point

**Files:**
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (replace placeholder `run()` with real logic)
- Modify: `crates/cli/src/commands/loadtest/dispatcher.rs` (accept tier weights from config)

Connect all the pieces: parse config -> build strategy -> run dispatcher -> collect metrics -> print report -> optionally write JSON -> exit with appropriate code.

- [ ] **Step 1: Write integration test for the full dev-bypass pipeline**

Add to `crates/cli/src/commands/loadtest/mod.rs` tests:

```rust
#[cfg(test)]
mod integration_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn test_full_dev_bypass_pipeline() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "ok"}}]
            })))
            .mount(&mock)
            .await;

        let args = LoadTestArgs {
            rps: 5,
            duration: "1s".to_string(),
            concurrency: 10,
            mode: "dev-bypass".to_string(),
            tier_weights: None,
            slo_p99_ms: 5000,
            slo_error_rate: 0.10,
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };

        let result = run(&mock.uri(), args).await;
        assert!(result.is_ok(), "full pipeline should succeed: {:?}", result.err());
    }

    #[tokio::test]
    async fn test_dry_run_does_not_send_requests() {
        // Use a dead URL — if dry_run works, no connection attempt is made.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dead_url = format!("http://127.0.0.1:{port}");

        let args = LoadTestArgs {
            rps: 100,
            duration: "60s".to_string(),
            concurrency: 50,
            mode: "dev-bypass".to_string(),
            tier_weights: None,
            slo_p99_ms: 1000,
            slo_error_rate: 0.01,
            report_json: None,
            prometheus_url: None,
            dry_run: true,
        };

        let result = run(&dead_url, args).await;
        assert!(result.is_ok(), "dry run should succeed without connecting");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli integration_tests -- --nocapture 2>&1 | head -30`
Expected: The `test_full_dev_bypass_pipeline` test hits the "not yet implemented" path and succeeds trivially. We need to make it actually use the dispatcher.

- [ ] **Step 3: Update run() to wire dispatcher, metrics, and report**

Replace the placeholder `run()` in `crates/cli/src/commands/loadtest/mod.rs` with:

```rust
pub async fn run(api_url: &str, args: LoadTestArgs) -> Result<()> {
    let config = args.into_config(api_url)?;

    if config.dry_run {
        println!("=== DRY RUN ===");
        println!("Target:       {}", config.api_url);
        println!("Mode:         {:?}", config.mode);
        println!("RPS:          {}", config.rps);
        println!("Duration:     {}s", config.duration_secs);
        println!("Concurrency:  {}", config.concurrency);
        println!("Total reqs:   {}", config.total_requests());
        println!("Tier weights: simple={}, medium={}, complex={}, reasoning={}",
            config.tier_weights.simple,
            config.tier_weights.medium,
            config.tier_weights.complex,
            config.tier_weights.reasoning,
        );
        println!("SLO p99:      {}ms", config.slo.p99_ms);
        println!("SLO err rate: {:.2}%", config.slo.error_rate * 100.0);
        if let Some(ref path) = config.report_json {
            println!("JSON report:  {path}");
        }
        if let Some(ref url) = config.prometheus_url {
            println!("Prometheus:   {url}");
        }
        println!("=== No requests sent ===");
        return Ok(());
    }

    // Resolve Solana RPC URL for payment modes.
    let rpc_url = match config.mode {
        config::LoadTestMode::Exact | config::LoadTestMode::Escrow => {
            std::env::var("SOLANA_RPC_URL")
                .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
                .map_err(|_| anyhow::anyhow!(
                    "SOLANA_RPC_URL required for {:?} mode payment signing", config.mode
                ))?
        }
        config::LoadTestMode::DevBypass => String::new(),
    };

    // Build payment strategy.
    let strategy: std::sync::Arc<dyn payment::PaymentStrategy> = match config.mode {
        config::LoadTestMode::DevBypass => std::sync::Arc::new(payment::DevBypassStrategy),
        config::LoadTestMode::Exact | config::LoadTestMode::Escrow => {
            // Exact and Escrow strategies will be added in Tasks 8-9.
            // For now, fall back to dev-bypass with a warning.
            eprintln!("Warning: {:?} mode not yet implemented, falling back to dev-bypass", config.mode);
            std::sync::Arc::new(payment::DevBypassStrategy)
        }
    };

    let mode_name = strategy.name().to_string();

    println!("Starting load test: {} RPS x {}s = {} requests (mode: {}, concurrency: {})",
        config.rps, config.duration_secs, config.total_requests(), mode_name, config.concurrency);

    let metrics = std::sync::Arc::new(metrics::MetricsCollector::new());

    let dispatcher_config = dispatcher::DispatcherConfig {
        api_url: config.api_url.clone(),
        rpc_url,
        rps: config.rps,
        duration_secs: config.duration_secs,
        concurrency: config.concurrency,
    };

    dispatcher::run_dispatcher(dispatcher_config, strategy, metrics.clone()).await;

    // Collect and display results.
    let snapshot = metrics.snapshot();
    report::print_terminal_report(&snapshot, &config.slo, config.duration_secs, &mode_name);

    // Write JSON report if requested.
    if let Some(ref json_path) = config.report_json {
        let json_report = report::build_json_report(&snapshot, &config.slo, config.duration_secs, &mode_name);
        let json_str = serde_json::to_string_pretty(&json_report)
            .context("failed to serialize JSON report")?;
        std::fs::write(json_path, &json_str)
            .with_context(|| format!("failed to write JSON report to {json_path}"))?;
        println!("JSON report written to: {json_path}");
    }

    // Exit with error if SLO violated (useful for CI/CD).
    let slo_result = report::check_slo(&snapshot, &config.slo);
    if !slo_result.passed {
        return Err(anyhow::anyhow!("SLO violated: {}", slo_result.violations.join("; ")));
    }

    Ok(())
}
```

- [ ] **Step 4: Update dispatcher to accept tier weights**

In `crates/cli/src/commands/loadtest/dispatcher.rs`, add `tier_weights` to `DispatcherConfig`:

```rust
pub struct DispatcherConfig {
    pub api_url: String,
    pub rpc_url: String,
    pub rps: u64,
    pub duration_secs: u64,
    pub concurrency: usize,
    pub tier_weights: super::config::TierWeights,
}
```

Update `run_dispatcher` to use `config.tier_weights` instead of the hardcoded default. Update the `DispatcherConfig` construction in `mod.rs` to pass `config.tier_weights.clone()`.

- [ ] **Step 5: Run all loadtest tests**

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture`
Expected: All tests pass (config, metrics, payment, worker, dispatcher, report, integration).

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/
git commit -m "feat: wire load test dispatcher, metrics, and report into rcr loadtest entry point"
```

---

## Task 8: ExactPayment Strategy (SPL TransferChecked)

**Files:**
- Modify: `crates/cli/src/commands/loadtest/payment.rs` (add `ExactPaymentStrategy`)
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (update strategy construction)

Implements the `PaymentStrategy` for exact mode. Reuses `build_usdc_transfer` from `solana_tx.rs` and constructs the full `PaymentPayload` with base64 encoding.

- [ ] **Step 1: Write failing tests**

Add to `crates/cli/src/commands/loadtest/payment.rs` tests:

```rust
#[tokio::test]
async fn test_exact_strategy_produces_header() {
    // This test requires a mock Solana RPC for blockhash.
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": {
                    "blockhash": "11111111111111111111111111111111",
                    "lastValidBlockHeight": 9999
                }
            }
        })))
        .mount(&mock)
        .await;

    let seed = [42u8; 32];
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let mut full = [0u8; 64];
    full[..32].copy_from_slice(&seed);
    full[32..].copy_from_slice(verifying_key.as_bytes());
    let keypair_b58 = bs58::encode(&full).into_string();

    let strategy = ExactPaymentStrategy::new(keypair_b58, reqwest::Client::new());

    let accepts = vec![x402::types::PaymentAccept {
        scheme: "exact".to_string(),
        network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
        amount: "1000".to_string(),
        asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
        pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
        max_timeout_seconds: 300,
        escrow_program_id: None,
    }];

    let result = strategy
        .prepare_payment(&mock.uri(), &serde_json::json!({}), &accepts)
        .await
        .expect("exact payment should succeed");

    assert!(result.is_some(), "exact strategy should produce a payment header");

    // Verify the header is valid base64 that decodes to a PaymentPayload.
    let header = result.unwrap();
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&header)
        .expect("header should be valid base64");
    let payload: serde_json::Value =
        serde_json::from_slice(&decoded).expect("should be valid JSON");
    assert_eq!(payload["accepted"]["scheme"], "exact");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli test_exact_strategy -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `ExactPaymentStrategy` not defined.

- [ ] **Step 3: Implement ExactPaymentStrategy**

Add to `crates/cli/src/commands/loadtest/payment.rs`:

```rust
use anyhow::Context;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use secrecy::{ExposeSecret, SecretString};
use x402::types::{PayloadData, PaymentPayload, Resource, SolanaPayload};

/// Exact payment strategy — builds and signs a real SPL TransferChecked
/// transaction per request.
///
/// The keypair is wrapped in `SecretString` to prevent accidental logging
/// and to enable zeroization on drop.
pub struct ExactPaymentStrategy {
    keypair_b58: SecretString,
    rpc_client: reqwest::Client,
}

impl std::fmt::Debug for ExactPaymentStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExactPaymentStrategy")
            .field("keypair_b58", &"[REDACTED]")
            .finish()
    }
}

impl ExactPaymentStrategy {
    pub fn new(keypair_b58: String, rpc_client: reqwest::Client) -> Self {
        Self {
            keypair_b58: SecretString::new(keypair_b58),
            rpc_client,
        }
    }
}

#[async_trait::async_trait]
impl PaymentStrategy for ExactPaymentStrategy {
    fn name(&self) -> &'static str {
        "exact"
    }

    async fn prepare_payment(
        &self,
        rpc_url: &str,
        _request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        let accepted = accepts
            .iter()
            .find(|a| a.scheme == "exact")
            .context("no 'exact' scheme in accepts list")?
            .clone();

        let amount: u64 = accepted.amount.parse()
            .context("invalid payment amount")?;

        let signed_tx = crate::commands::solana_tx::build_usdc_transfer(
            self.keypair_b58.expose_secret(),
            &accepted.pay_to,
            amount,
            rpc_url,
            &self.rpc_client,
        )
        .await
        .context("failed to build USDC transfer for exact payment")?;

        let payload = PaymentPayload {
            x402_version: x402::types::X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted,
            payload: PayloadData::Direct(SolanaPayload {
                transaction: signed_tx,
            }),
        };

        let json = serde_json::to_string(&payload)?;
        Ok(Some(BASE64.encode(json.as_bytes())))
    }
}
```

- [ ] **Step 4: Update mod.rs strategy construction**

In `crates/cli/src/commands/loadtest/mod.rs`, update the `Exact` arm:

```rust
config::LoadTestMode::Exact => {
    let wallet = crate::commands::wallet::load_wallet()
        .context("wallet required for exact payment mode — run `rcr wallet init` first")?;
    let keypair_b58 = wallet["private_key"]
        .as_str()
        .context("wallet missing private_key")?
        .to_string();
    let rpc_client = reqwest::Client::new();
    std::sync::Arc::new(payment::ExactPaymentStrategy::new(keypair_b58, rpc_client))
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli test_exact_strategy -- --nocapture`
Expected: Test passes — header is valid base64/JSON PaymentPayload.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/payment.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add ExactPaymentStrategy with SPL TransferChecked signing"
```

---

## Task 9: EscrowPayment Strategy (Anchor Deposit)

**Files:**
- Modify: `crates/cli/src/commands/loadtest/payment.rs` (add `EscrowPaymentStrategy`)
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (update Escrow arm)

Implements the `PaymentStrategy` for escrow mode. Reuses `build_escrow_deposit` and `fetch_current_slot` from `solana_tx.rs`. Generates unique service IDs per request.

- [ ] **Step 1: Write failing tests**

Add to `crates/cli/src/commands/loadtest/payment.rs` tests:

```rust
#[tokio::test]
async fn test_escrow_strategy_produces_header() {
    use wiremock::matchers::{method, path, body_string_contains};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let mock = MockServer::start().await;

    // Mock getSlot.
    Mock::given(method("POST"))
        .and(path("/"))
        .and(body_string_contains("getSlot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "result": 12345
        })))
        .mount(&mock)
        .await;

    // Mock getLatestBlockhash.
    Mock::given(method("POST"))
        .and(path("/"))
        .and(body_string_contains("getLatestBlockhash"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "value": { "blockhash": "11111111111111111111111111111111", "lastValidBlockHeight": 9999 } }
        })))
        .mount(&mock)
        .await;

    let seed = [42u8; 32];
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let verifying_key = signing_key.verifying_key();
    let mut full = [0u8; 64];
    full[..32].copy_from_slice(&seed);
    full[32..].copy_from_slice(verifying_key.as_bytes());
    let keypair_b58 = bs58::encode(&full).into_string();

    let strategy = EscrowPaymentStrategy::new(keypair_b58, reqwest::Client::new());

    let accepts = vec![x402::types::PaymentAccept {
        scheme: "escrow".to_string(),
        network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
        amount: "1000".to_string(),
        asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
        pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
        max_timeout_seconds: 300,
        escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
    }];

    let result = strategy
        .prepare_payment(&mock.uri(), &serde_json::json!({"model": "auto"}), &accepts)
        .await
        .expect("escrow payment should succeed");

    assert!(result.is_some(), "escrow strategy should produce a payment header");

    let header = result.unwrap();
    let decoded = base64::engine::general_purpose::STANDARD.decode(&header).expect("base64");
    let payload: serde_json::Value = serde_json::from_slice(&decoded).expect("json");
    assert_eq!(payload["accepted"]["scheme"], "escrow");
    assert!(payload["payload"]["deposit_tx"].is_string(), "should contain deposit_tx");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli test_escrow_strategy -- --nocapture 2>&1 | head -20`
Expected: Compilation error — `EscrowPaymentStrategy` not defined.

- [ ] **Step 3: Implement EscrowPaymentStrategy**

Add to `crates/cli/src/commands/loadtest/payment.rs`:

```rust
use sha2::{Digest, Sha256};
use x402::types::EscrowPayload;

/// Generate a unique 32-byte service_id by hashing the request body + random nonce.
fn generate_service_id(request_body: &[u8]) -> Result<[u8; 32]> {
    let mut hasher = Sha256::new();
    hasher.update(request_body);
    let mut nonce = [0u8; 8];
    getrandom::getrandom(&mut nonce).context("getrandom failed")?;
    hasher.update(nonce);
    let hash = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash);
    Ok(id)
}

/// Escrow payment strategy — builds a real Anchor deposit transaction per request.
///
/// The keypair is wrapped in `SecretString` to prevent accidental logging
/// and to enable zeroization on drop.
pub struct EscrowPaymentStrategy {
    keypair_b58: SecretString,
    rpc_client: reqwest::Client,
}

impl std::fmt::Debug for EscrowPaymentStrategy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EscrowPaymentStrategy")
            .field("keypair_b58", &"[REDACTED]")
            .finish()
    }
}

impl EscrowPaymentStrategy {
    pub fn new(keypair_b58: String, rpc_client: reqwest::Client) -> Self {
        Self {
            keypair_b58: SecretString::new(keypair_b58),
            rpc_client,
        }
    }
}

#[async_trait::async_trait]
impl PaymentStrategy for EscrowPaymentStrategy {
    fn name(&self) -> &'static str {
        "escrow"
    }

    async fn prepare_payment(
        &self,
        rpc_url: &str,
        request_body: &serde_json::Value,
        accepts: &[PaymentAccept],
    ) -> Result<Option<String>> {
        let accepted = accepts
            .iter()
            .find(|a| a.scheme == "escrow" && a.escrow_program_id.is_some())
            .context("no 'escrow' scheme with program_id in accepts list")?
            .clone();

        let escrow_program_id = accepted
            .escrow_program_id
            .as_deref()
            .context("escrow scheme missing program_id")?;

        let amount: u64 = accepted.amount.parse()
            .context("invalid payment amount")?;

        let body_bytes = serde_json::to_vec(request_body)
            .context("failed to serialize request body")?;
        let service_id = generate_service_id(&body_bytes)?;

        let current_slot = crate::commands::solana_tx::fetch_current_slot(rpc_url, &self.rpc_client)
            .await
            .context("failed to fetch current slot")?;

        let timeout_slots = (accepted.max_timeout_seconds * 1000) / 400;
        let expiry_slot = current_slot + timeout_slots;

        let deposit_tx = crate::commands::solana_tx::build_escrow_deposit(
            self.keypair_b58.expose_secret(),
            &accepted.pay_to,
            escrow_program_id,
            amount,
            service_id,
            expiry_slot,
            rpc_url,
            &self.rpc_client,
        )
        .await
        .context("failed to build escrow deposit transaction")?;

        // Derive agent pubkey from keypair.
        let key_bytes = bs58::decode(self.keypair_b58.expose_secret())
            .into_vec()
            .context("keypair decode")?;
        let seed: [u8; 32] = key_bytes[..32]
            .try_into()
            .map_err(|_| anyhow::anyhow!("bad seed"))?;
        let agent_pubkey = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
        let agent_pubkey_b58 = bs58::encode(agent_pubkey.as_bytes()).into_string();

        let payload = PaymentPayload {
            x402_version: x402::types::X402_VERSION,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted,
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx,
                service_id: BASE64.encode(service_id),
                agent_pubkey: agent_pubkey_b58,
            }),
        };

        let json = serde_json::to_string(&payload)?;
        Ok(Some(BASE64.encode(json.as_bytes())))
    }
}
```

- [ ] **Step 4: Update mod.rs Escrow strategy arm**

In `crates/cli/src/commands/loadtest/mod.rs`, update the `Escrow` arm:

```rust
config::LoadTestMode::Escrow => {
    let wallet = crate::commands::wallet::load_wallet()
        .context("wallet required for escrow payment mode — run `rcr wallet init` first")?;
    let keypair_b58 = wallet["private_key"]
        .as_str()
        .context("wallet missing private_key")?
        .to_string();
    let rpc_client = reqwest::Client::new();
    std::sync::Arc::new(payment::EscrowPaymentStrategy::new(keypair_b58, rpc_client))
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli test_escrow_strategy -- --nocapture`
Expected: Test passes — header is valid base64/JSON PaymentPayload with escrow fields.

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture`
Expected: All loadtest module tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/payment.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add EscrowPaymentStrategy with Anchor deposit signing"
```

**Note on escrow claim verification:** After the gateway processes an escrow-paid request, it fires an async `tokio::spawn` claim transaction. Verifying that claims actually settled on-chain requires querying the Solana RPC for PDA state changes (`getProgramAccounts` filtered by agent pubkey). This is intentionally deferred to a future enhancement because: (1) claims are fire-and-forget with variable settlement latency, (2) querying Solana RPC under load adds significant complexity, and (3) the gateway already has unit/integration tests for claim logic. A follow-up task can add a `--verify-claims` flag that waits N seconds after the test completes, then queries Solana for all escrow PDAs belonging to the test wallet and reports how many were claimed vs. still pending vs. expired. The `crates/cli/src/commands/recover.rs` module already has the PDA parsing logic (`ESCROW_ACCOUNT_LEN`, offset constants, `getProgramAccounts` RPC calls) that can be reused for this.

---

## Task 10: Prometheus Scraper and SLO Validator

**Files:**
- Create: `crates/cli/src/commands/loadtest/prometheus.rs`
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add `pub mod prometheus;`, wire into run)

Optional Prometheus /metrics scraper that takes a snapshot before and after the load test, computes deltas, and validates that gateway-reported metrics match the load test's internal counts.

- [ ] **Step 1: Write failing tests**

In `crates/cli/src/commands/loadtest/prometheus.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_prometheus_line_counter() {
        let line = "rcr_http_requests_total{method=\"POST\",path=\"/v1/chat/completions\"} 42";
        let parsed = parse_metric_line(line);
        assert_eq!(parsed, Some(("rcr_http_requests_total".to_string(), 42.0)));
    }

    #[test]
    fn test_parse_prometheus_line_skips_comments() {
        let line = "# HELP rcr_http_requests_total Total requests";
        let parsed = parse_metric_line(line);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_compute_deltas() {
        let before = vec![
            ("rcr_http_requests_total".to_string(), 100.0),
            ("rcr_http_errors_total".to_string(), 5.0),
        ];
        let after = vec![
            ("rcr_http_requests_total".to_string(), 200.0),
            ("rcr_http_errors_total".to_string(), 8.0),
            ("rcr_new_metric".to_string(), 10.0),
        ];

        let deltas = compute_deltas(&before, &after);
        assert_eq!(deltas.get("rcr_http_requests_total"), Some(&100.0));
        assert_eq!(deltas.get("rcr_http_errors_total"), Some(&3.0));
        assert_eq!(deltas.get("rcr_new_metric"), Some(&10.0));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli prometheus -- --nocapture 2>&1 | head -20`
Expected: Compilation error.

- [ ] **Step 3: Implement Prometheus scraper**

In `crates/cli/src/commands/loadtest/prometheus.rs`:

```rust
use std::collections::HashMap;

use anyhow::{Context, Result};

/// Parse a single Prometheus exposition format line into (metric_name, value).
/// Returns None for comments, TYPE lines, and unparseable lines.
pub fn parse_metric_line(line: &str) -> Option<(String, f64)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }
    // Format: metric_name{labels} value [timestamp]
    // or:     metric_name value [timestamp]
    let name_end = line.find('{').or_else(|| line.find(' '))?;
    let name = &line[..name_end];

    // Find the value after the last '}' or the first ' '.
    let value_start = if let Some(brace_end) = line.find('}') {
        brace_end + 2 // skip '} '
    } else {
        name_end + 1
    };

    if value_start >= line.len() {
        return None;
    }

    let value_str = line[value_start..].split_whitespace().next()?;
    let value: f64 = value_str.parse().ok()?;
    Some((name.to_string(), value))
}

/// Scrape a Prometheus /metrics endpoint and return parsed metric name-value pairs.
pub async fn scrape_metrics(url: &str) -> Result<Vec<(String, f64)>> {
    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to scrape Prometheus at {url}"))?;

    let text = resp
        .text()
        .await
        .context("failed to read Prometheus response body")?;

    let metrics: Vec<(String, f64)> = text
        .lines()
        .filter_map(parse_metric_line)
        .collect();

    Ok(metrics)
}

/// Compute deltas between before and after metric snapshots.
pub fn compute_deltas(
    before: &[(String, f64)],
    after: &[(String, f64)],
) -> HashMap<String, f64> {
    let before_map: HashMap<&str, f64> = before.iter().map(|(k, v)| (k.as_str(), *v)).collect();

    after
        .iter()
        .map(|(name, after_val)| {
            let before_val = before_map.get(name.as_str()).copied().unwrap_or(0.0);
            (name.clone(), after_val - before_val)
        })
        .collect()
}

/// Print a summary of Prometheus metric deltas.
pub fn print_prometheus_deltas(deltas: &HashMap<String, f64>) {
    if deltas.is_empty() {
        println!("  (no Prometheus metrics found)");
        return;
    }

    println!();
    println!("  --- Prometheus Deltas ---");

    let mut sorted: Vec<_> = deltas.iter().collect();
    sorted.sort_by_key(|(k, _)| k.clone());

    for (name, delta) in sorted {
        if *delta != 0.0 {
            println!("  {name}: {delta:+.0}");
        }
    }
}
```

- [ ] **Step 4: Add module declaration and wire into run()**

In `crates/cli/src/commands/loadtest/mod.rs`, add:
```rust
pub mod prometheus;
```

In the `run()` function, add Prometheus scraping before and after the dispatcher call:

```rust
// Before dispatcher:
let prom_before = if let Some(ref prom_url) = config.prometheus_url {
    Some(prometheus::scrape_metrics(prom_url).await
        .context("failed to scrape Prometheus before test")?)
} else {
    None
};

// ... run dispatcher ...

// After dispatcher, before report:
if let (Some(ref prom_url), Some(before)) = (&config.prometheus_url, &prom_before) {
    let after = prometheus::scrape_metrics(prom_url).await
        .context("failed to scrape Prometheus after test")?;
    let deltas = prometheus::compute_deltas(before, &after);
    prometheus::print_prometheus_deltas(&deltas);
}
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p rustyclawrouter-cli prometheus -- --nocapture`
Expected: All 3 prometheus tests pass.

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture`
Expected: All loadtest tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/cli/src/commands/loadtest/prometheus.rs crates/cli/src/commands/loadtest/mod.rs
git commit -m "feat: add Prometheus metrics scraper with delta computation for SLO validation"
```

---

## Task 11: Final Integration and Full Test Suite

**Files:**
- Modify: `crates/cli/src/commands/loadtest/mod.rs` (add comprehensive integration tests)

End-to-end integration tests that verify the full pipeline with wiremock: dev-bypass, 402 payment dance, rate limiting, JSON report output, and SLO failure.

- [ ] **Step 1: Write integration tests**

Add to the existing `integration_tests` module in `crates/cli/src/commands/loadtest/mod.rs`:

```rust
#[tokio::test]
async fn test_json_report_output() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .mount(&mock)
        .await;

    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let json_path = tmp.path().to_str().expect("path").to_string();

    let args = LoadTestArgs {
        rps: 5,
        duration: "1s".to_string(),
        concurrency: 10,
        mode: "dev-bypass".to_string(),
        tier_weights: None,
        slo_p99_ms: 10000,
        slo_error_rate: 0.50,
        report_json: Some(json_path.clone()),
        prometheus_url: None,
        dry_run: false,
    };

    let result = run(&mock.uri(), args).await;
    assert!(result.is_ok(), "pipeline should succeed: {:?}", result.err());

    // Verify JSON file was written and is valid.
    let contents = std::fs::read_to_string(&json_path).expect("read JSON");
    let report: serde_json::Value = serde_json::from_str(&contents).expect("parse JSON");
    assert_eq!(report["mode"], "dev-bypass");
    assert!(report["metrics"]["total_requests"].as_u64().unwrap_or(0) > 0);
}

#[tokio::test]
async fn test_slo_failure_returns_error() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("error"))
        .mount(&mock)
        .await;

    let args = LoadTestArgs {
        rps: 5,
        duration: "1s".to_string(),
        concurrency: 10,
        mode: "dev-bypass".to_string(),
        tier_weights: None,
        slo_p99_ms: 5000,
        slo_error_rate: 0.01, // Very strict — will fail with 100% errors.
        report_json: None,
        prometheus_url: None,
        dry_run: false,
    };

    let result = run(&mock.uri(), args).await;
    assert!(result.is_err(), "should fail when SLO is violated");
    assert!(
        result.unwrap_err().to_string().contains("SLO violated"),
        "error should mention SLO violation"
    );
}

#[tokio::test]
async fn test_custom_tier_weights() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "ok"}}]
        })))
        .mount(&mock)
        .await;

    let args = LoadTestArgs {
        rps: 5,
        duration: "1s".to_string(),
        concurrency: 10,
        mode: "dev-bypass".to_string(),
        tier_weights: Some("simple=100,medium=0,complex=0,reasoning=0".to_string()),
        slo_p99_ms: 10000,
        slo_error_rate: 0.50,
        report_json: None,
        prometheus_url: None,
        dry_run: false,
    };

    let result = run(&mock.uri(), args).await;
    assert!(result.is_ok(), "custom tier weights should work");
}
```

- [ ] **Step 2: Run the full test suite**

Run: `cargo test -p rustyclawrouter-cli loadtest -- --nocapture`
Expected: All tests pass (config, metrics, payment, worker, dispatcher, report, prometheus, integration).

Run: `cargo clippy -p rustyclawrouter-cli --all-targets -- -D warnings`
Expected: No warnings.

Run: `cargo fmt -p rustyclawrouter-cli -- --check`
Expected: No formatting issues.

- [ ] **Step 3: Verify the binary compiles and --help works**

Run: `cargo build -p rustyclawrouter-cli`
Expected: Clean build.

Run: `cargo run -p rustyclawrouter-cli -- loadtest --help`
Expected: Help text showing all loadtest arguments.

Run: `cargo run -p rustyclawrouter-cli -- loadtest --dry-run --rps 100 --duration 30s --mode exact`
Expected: Dry run output showing the config preview.

- [ ] **Step 4: Commit**

```bash
git add crates/cli/src/commands/loadtest/
git commit -m "feat: add integration tests for load test pipeline with SLO validation"
```

---

## Dependency Summary

**New dependency added to `crates/cli/Cargo.toml`:**
- `hdrhistogram = "7"` — latency percentile tracking
- `secrecy = "0.8"` — secret wrapping for keypair fields with zeroization on drop
- `async-trait = { workspace = true }` — async trait for PaymentStrategy (if not already present)
- `sha2 = { workspace = true }` — already in Cargo.toml for service_id generation
- `getrandom = "0.2"` — already in Cargo.toml

**No new workspace-level dependencies.** All other deps (tokio, reqwest, clap, serde_json, chrono, base64, bs58, ed25519-dalek) are already available.

## Test Count Summary

| Module | Tests |
|---|---|
| config.rs | 5 (weights, validation) |
| metrics.rs | 8 (counters, percentiles, error rate, dropped requests) |
| payment.rs | 4 (dev-bypass, exact, escrow, display name) |
| worker.rs | 5 (200, 5xx, 429, connection error, 402-dance with payment) |
| dispatcher.rs | 2 (request count, concurrency limit + dropped verification) |
| report.rs | 5 (SLO pass/fail, JSON, terminal) |
| prometheus.rs | 3 (parsing, comments, deltas) |
| mod.rs integration | 5 (full pipeline, dry-run, JSON output, SLO failure, custom weights) |
| **Total** | **37** |
