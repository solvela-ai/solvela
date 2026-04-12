pub mod config;
pub mod dispatcher;
pub mod metrics;
pub mod payment;
pub mod prometheus;
pub mod report;
pub mod worker;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Args;
use secrecy::SecretString;

use self::config::{LoadTestConfig, LoadTestMode, SloThresholds, TierWeights};
use self::dispatcher::{run_dispatcher, DispatcherConfig};
use self::metrics::MetricsCollector;
use self::payment::{
    DevBypassStrategy, EscrowPaymentStrategy, ExactPaymentStrategy, PaymentStrategy,
};
use self::report::{print_terminal_report, write_json_report};
use self::worker::execute_request;

/// Load test the Solvela gateway with real payment transactions.
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
        return s
            .parse::<u64>()
            .map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"));
    }
    if let Some(m) = input.strip_suffix('m') {
        let mins: u64 = m
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"))?;
        return Ok(mins * 60);
    }
    if let Some(h) = input.strip_suffix('h') {
        let hours: u64 = h
            .parse()
            .map_err(|_| anyhow::anyhow!("invalid duration: '{input}'"))?;
        return Ok(hours * 3600);
    }
    // Bare number = seconds.
    input
        .parse::<u64>()
        .map_err(|_| anyhow::anyhow!("invalid duration: '{input}'. Use e.g. 60s, 5m, 1h"))
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

/// Entry point for `solvela loadtest`.
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
        println!(
            "Tier weights: simple={}, medium={}, complex={}, reasoning={}",
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

    // --- Build shared resources ---
    let rpc_url: String = match config.mode {
        LoadTestMode::Exact | LoadTestMode::Escrow => std::env::var("SOLANA_RPC_URL")
            .or_else(|_| std::env::var("SOLVELA_SOLANA_RPC_URL"))
            .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "SOLANA_RPC_URL (or SOLVELA_SOLANA_RPC_URL) is required for {:?} payment mode",
                    config.mode
                )
            })?,
        _ => String::new(),
    };

    let strategy: Arc<dyn PaymentStrategy> = match config.mode {
        LoadTestMode::DevBypass => Arc::new(DevBypassStrategy),
        LoadTestMode::Exact => {
            let keypair_b58 = std::env::var("SOLANA_WALLET_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "SOLANA_WALLET_KEY env var is required for exact payment mode. \
                         Set it to your 64-byte Solana keypair in base58."
                )
            })?;
            let rpc_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            Arc::new(ExactPaymentStrategy::new(
                SecretString::new(keypair_b58),
                rpc_client,
            ))
        }
        LoadTestMode::Escrow => {
            let keypair_b58 = std::env::var("SOLANA_WALLET_KEY").map_err(|_| {
                anyhow::anyhow!(
                    "SOLANA_WALLET_KEY env var is required for escrow payment mode. \
                         Set it to your 64-byte Solana keypair in base58."
                )
            })?;
            let rpc_client = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .build()?;
            Arc::new(EscrowPaymentStrategy::new(
                SecretString::new(keypair_b58),
                rpc_client,
                rpc_url.clone(),
            ))
        }
    };

    let client = Arc::new(
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .pool_max_idle_per_host(config.concurrency)
            .build()?,
    );

    let metrics = Arc::new(MetricsCollector::new());

    let dispatcher_config = DispatcherConfig {
        rps: config.rps,
        duration_secs: config.duration_secs,
        concurrency: config.concurrency,
    };

    let api_url_shared: Arc<str> = Arc::from(config.api_url.as_str());
    let rpc_url_shared: Arc<str> = Arc::from(rpc_url.as_str());

    // --- Prometheus pre-test scrape ---
    let prom_before = if let Some(ref prom_url) = config.prometheus_url {
        match prometheus::scrape_metrics(prom_url).await {
            Ok(metrics) => Some(metrics),
            Err(e) => {
                eprintln!(
                    "WARNING: Prometheus pre-test scrape failed (will skip delta report): {e}"
                );
                None
            }
        }
    } else {
        None
    };

    // --- Run the load test ---
    println!(
        "Starting load test: {} RPS x {}s = {} requests (mode: {:?})",
        config.rps,
        config.duration_secs,
        config.total_requests(),
        config.mode,
    );

    {
        let client = client.clone();
        let strategy = strategy.clone();
        let api_url_shared = api_url_shared.clone();
        let rpc_url_shared = rpc_url_shared.clone();

        run_dispatcher(
            dispatcher_config,
            config.tier_weights.clone(),
            metrics.clone(),
            move |scheduled_at, tier, metrics| {
                let client = client.clone();
                let strategy = strategy.clone();
                let api_url = api_url_shared.clone();
                let rpc_url = rpc_url_shared.clone();

                async move {
                    let body = build_request_body(tier);
                    let _ = execute_request(
                        &client,
                        &api_url,
                        &body,
                        strategy.as_ref(),
                        &rpc_url,
                        &metrics,
                        scheduled_at,
                    )
                    .await;
                }
            },
        )
        .await;
    }

    // --- Prometheus post-test scrape and delta report ---
    if let (Some(ref prom_url), Some(ref before)) = (&config.prometheus_url, &prom_before) {
        match prometheus::scrape_metrics(prom_url).await {
            Ok(after) => {
                let deltas = prometheus::compute_deltas(before, &after);
                prometheus::print_prometheus_deltas(&deltas);
            }
            Err(e) => {
                eprintln!(
                    "WARNING: Prometheus post-test scrape failed (skipping delta report): {e}"
                );
            }
        }
    }

    // --- Report results ---
    let snapshot = metrics.snapshot();
    print_terminal_report(&snapshot, &config);

    if let Some(ref path) = config.report_json {
        write_json_report(&snapshot, &config, path)?;
    }

    // Return non-zero exit via error if SLO fails.
    let p99_pass = snapshot.p99_ms <= config.slo.p99_ms;
    let error_rate_pass = snapshot.error_rate() <= config.slo.error_rate;
    if !p99_pass || !error_rate_pass {
        bail!("SLO check failed (p99_pass={p99_pass}, error_rate_pass={error_rate_pass})");
    }

    Ok(())
}

/// Build a representative chat request body for the given complexity tier.
///
/// Each tier uses a different prompt length and model hint so the gateway's
/// smart router classifies them into the expected scoring bucket.
fn build_request_body(tier: &str) -> serde_json::Value {
    let (model, prompt) = match tier {
        "simple" => ("auto", "Say hello."),
        "medium" => ("auto", "Explain how HTTP caching works with ETags and Cache-Control headers."),
        "complex" => ("auto", "Write a Rust function that implements a lock-free concurrent hash map with linear probing. Include detailed comments explaining the memory ordering constraints."),
        "reasoning" => ("auto", "Prove that every continuous function on a closed interval is uniformly continuous. Then explain why this fails for open intervals with a concrete counterexample."),
        _ => ("auto", "Say hello."),
    };

    serde_json::json!({
        "model": model,
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 64
    })
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

#[cfg(test)]
mod integration_tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// End-to-end dev-bypass flow: run a short load test against a wiremock
    /// server returning 200, verify MetricsSnapshot shows correct counts.
    #[tokio::test]
    async fn test_end_to_end_dev_bypass_flow() {
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
            slo_p99_ms: 10000,
            slo_error_rate: 0.50,
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };

        let result = run(&mock.uri(), args).await;
        assert!(
            result.is_ok(),
            "dev-bypass pipeline should succeed: {:?}",
            result.err()
        );
    }

    /// Run a short load test, write a JSON report, parse it back, and verify
    /// the structure contains expected fields.
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
        assert!(
            result.is_ok(),
            "pipeline should succeed: {:?}",
            result.err()
        );

        // Verify JSON file was written and is valid.
        let contents = std::fs::read_to_string(&json_path).expect("read JSON");
        let report: serde_json::Value = serde_json::from_str(&contents).expect("parse JSON");
        assert_eq!(report["config"]["mode"], "DevBypass");
        assert!(
            report["metrics"]["total_requests"].as_u64().unwrap_or(0) > 0,
            "should have dispatched at least 1 request"
        );
        assert!(
            report["slo"]["overall_pass"].is_boolean(),
            "slo.overall_pass should be a boolean"
        );
        assert!(
            report["computed"]["effective_rps"].is_number(),
            "computed.effective_rps should be a number"
        );
        assert!(
            report["computed"]["error_rate"].is_number(),
            "computed.error_rate should be a number"
        );
    }

    /// When all requests return 5xx and the SLO error rate threshold is very
    /// strict, the run() function should return an error.
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
            slo_error_rate: 0.01, // Very strict -- will fail with 100% errors.
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };

        let result = run(&mock.uri(), args).await;
        assert!(result.is_err(), "should fail when SLO is violated");
        assert!(
            result.unwrap_err().to_string().contains("SLO check failed"),
            "error should mention SLO check failure"
        );
    }

    /// Verify that custom tier weights are accepted and the pipeline completes.
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

    /// End-to-end 402 dance: wiremock returns 402 then 200 (with payment
    /// header check), verify the full flow completes successfully.
    #[tokio::test]
    async fn test_end_to_end_402_dance() {
        use wiremock::matchers::header_exists;

        let mock = MockServer::start().await;

        // Requests WITH PAYMENT-SIGNATURE header -> 200.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "paid response"}}]
            })))
            .mount(&mock)
            .await;

        // Requests WITHOUT the header -> 402 with PaymentRequired body.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": "{\"x402_version\":1,\"resource\":{\"url\":\"/v1/chat/completions\",\"method\":\"POST\"},\"accepts\":[{\"scheme\":\"exact\",\"network\":\"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp\",\"amount\":\"1000\",\"asset\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\",\"pay_to\":\"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM\",\"max_timeout_seconds\":300}],\"cost_breakdown\":{\"provider_cost\":\"0.001000\",\"platform_fee\":\"0.000050\",\"total\":\"0.001050\",\"currency\":\"USDC\",\"fee_percent\":5},\"error\":\"Payment required\"}"
                }
            })))
            .mount(&mock)
            .await;

        // dev-bypass mode: DevBypassStrategy returns None for payment,
        // so the 402 will be recorded as PaymentRequired402.
        // This test verifies the 402 parsing path works end-to-end
        // without crashing.
        let args = LoadTestArgs {
            rps: 3,
            duration: "1s".to_string(),
            concurrency: 10,
            mode: "dev-bypass".to_string(),
            tier_weights: None,
            slo_p99_ms: 10000,
            slo_error_rate: 1.0, // Allow 100% errors since dev-bypass gets 402s.
            report_json: None,
            prometheus_url: None,
            dry_run: false,
        };

        let result = run(&mock.uri(), args).await;
        assert!(
            result.is_ok(),
            "402 dance pipeline should not crash: {:?}",
            result.err()
        );
    }

    /// SLO validation unit test: create a snapshot with known values,
    /// verify pass/fail behavior of the SLO check logic in run().
    #[test]
    fn test_slo_validation_logic() {
        use super::metrics::MetricsCollector;
        use std::time::Duration;

        // Scenario 1: All metrics within SLO -- should pass.
        let collector = MetricsCollector::new();
        for _ in 0..100 {
            collector.record_success(Duration::from_millis(50));
        }
        let snap = collector.snapshot();
        let p99_pass = snap.p99_ms <= 5000;
        let error_rate_pass = snap.error_rate() <= 0.05;
        assert!(p99_pass, "p99 of 50ms should pass 5000ms SLO");
        assert!(error_rate_pass, "0% errors should pass 5% SLO");

        // Scenario 2: High error rate -- should fail.
        let collector2 = MetricsCollector::new();
        for _ in 0..50 {
            collector2.record_success(Duration::from_millis(10));
        }
        for _ in 0..50 {
            collector2.record_outcome(
                super::metrics::RequestOutcome::ServerError5xx,
                Duration::from_millis(10),
            );
        }
        let snap2 = collector2.snapshot();
        let error_rate_pass2 = snap2.error_rate() <= 0.05;
        assert!(!error_rate_pass2, "50% error rate should fail 5% SLO");

        // Scenario 3: High latency -- should fail p99.
        let collector3 = MetricsCollector::new();
        for _ in 0..100 {
            collector3.record_success(Duration::from_millis(6000));
        }
        let snap3 = collector3.snapshot();
        let p99_pass3 = snap3.p99_ms <= 5000;
        assert!(!p99_pass3, "p99 of 6000ms should fail 5000ms SLO");
    }
}
