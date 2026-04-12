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
            other => Err(anyhow!(
                "unknown load test mode: '{other}'. Use: dev-bypass, exact, escrow"
            )),
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
        let mut weights = Self {
            simple: 0,
            medium: 0,
            complex: 0,
            reasoning: 0,
        };
        for pair in input.split(',') {
            let (key, val) = pair
                .split_once('=')
                .with_context(|| format!("invalid tier weight pair: '{pair}'"))?;
            let v: u8 = val
                .trim()
                .parse()
                .with_context(|| format!("invalid weight value: '{val}'"))?;
            match key.trim() {
                "simple" => weights.simple = v,
                "medium" => weights.medium = v,
                "complex" => weights.complex = v,
                "reasoning" => weights.reasoning = v,
                other => return Err(anyhow!("unknown tier: '{other}'")),
            }
        }
        let sum = weights.simple as u16
            + weights.medium as u16
            + weights.complex as u16
            + weights.reasoning as u16;
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
    pub model: String,
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
            model: "auto".to_string(),
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
            model: "auto".to_string(),
        };
        assert!(config.validate().is_err());
    }
}
