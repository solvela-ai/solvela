pub mod config;
pub mod dispatcher;
pub mod metrics;
pub mod payment;
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
            .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
            .map_err(|_| {
                anyhow::anyhow!(
                    "SOLANA_RPC_URL (or RCR_SOLANA_RPC_URL) is required for {:?} payment mode",
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
