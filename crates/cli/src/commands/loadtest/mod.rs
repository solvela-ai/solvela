pub mod config;
pub mod dispatcher;
pub mod metrics;

use anyhow::Result;
use clap::Args;

use self::config::{LoadTestConfig, LoadTestMode, SloThresholds, TierWeights};

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
