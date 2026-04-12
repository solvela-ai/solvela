//! SOL balance monitoring for fee-payer wallets.
//!
//! Periodically checks SOL balances via Solana RPC and emits structured
//! tracing warnings/errors when balances drop below configured thresholds.
//! This ensures operators are alerted before wallets run out of SOL for
//! transaction fees.

use std::sync::Arc;
use std::time::Duration;

use metrics::gauge;
use serde::Deserialize;
use tracing::{error, info, warn};

/// Lamports per SOL (1 SOL = 1_000_000_000 lamports).
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Alert severity based on SOL balance relative to thresholds.
#[derive(Debug, Clone, PartialEq)]
pub enum AlertLevel {
    Healthy,
    Warning,
    Critical,
}

/// Result of checking a single wallet's balance.
#[derive(Debug, Clone)]
pub struct BalanceCheckResult {
    pub wallet_pubkey: String,
    pub balance_sol: f64,
    pub lamports: u64,
    pub alert_level: AlertLevel,
}

/// Configuration for the balance monitor.
#[derive(Debug, Clone, Deserialize)]
pub struct MonitorConfig {
    /// SOL balance below which a warning is emitted (exclusive).
    #[serde(default = "default_warn_threshold")]
    pub warn_threshold_sol: f64,
    /// SOL balance below which a critical error is emitted (exclusive).
    #[serde(default = "default_critical_threshold")]
    pub critical_threshold_sol: f64,
    /// Seconds between balance checks.
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,
    /// Solana RPC URL used for getBalance calls.
    #[serde(default = "default_rpc_url")]
    pub rpc_url: String,
}

fn default_warn_threshold() -> f64 {
    0.1
}

fn default_critical_threshold() -> f64 {
    0.02
}

fn default_check_interval() -> u64 {
    300
}

fn default_rpc_url() -> String {
    "https://api.devnet.solana.com".to_string()
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            warn_threshold_sol: default_warn_threshold(),
            critical_threshold_sol: default_critical_threshold(),
            check_interval_secs: default_check_interval(),
            rpc_url: default_rpc_url(),
        }
    }
}

/// Format a lamport balance as a human-readable SOL string with 9 decimal places.
pub fn format_sol_balance(lamports: u64) -> String {
    let sol = lamports / LAMPORTS_PER_SOL;
    let remainder = lamports % LAMPORTS_PER_SOL;
    format!("{sol}.{remainder:09} SOL")
}

/// Background balance monitor that periodically checks SOL balances
/// of configured fee-payer wallets via the Solana JSON-RPC.
pub struct BalanceMonitor {
    config: MonitorConfig,
    wallet_pubkeys: Vec<String>,
    http_client: reqwest::Client,
}

impl BalanceMonitor {
    /// Create a new monitor for the given wallet pubkeys.
    pub fn new(config: MonitorConfig, wallet_pubkeys: Vec<String>) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();

        Self {
            config,
            wallet_pubkeys,
            http_client,
        }
    }

    /// Returns the number of wallets being monitored.
    pub fn wallet_count(&self) -> usize {
        self.wallet_pubkeys.len()
    }

    /// Determine alert level for a given SOL balance.
    ///
    /// Thresholds are exclusive:
    /// - `balance < critical` → `Critical`
    /// - `balance < warn` → `Warning`
    /// - otherwise → `Healthy`
    pub fn alert_level(&self, balance_sol: f64) -> AlertLevel {
        if balance_sol < self.config.critical_threshold_sol {
            AlertLevel::Critical
        } else if balance_sol < self.config.warn_threshold_sol {
            AlertLevel::Warning
        } else {
            AlertLevel::Healthy
        }
    }

    /// Check balances for all configured wallets via the Solana RPC.
    ///
    /// On RPC error for a specific wallet, logs a warning and skips it
    /// (never panics).
    pub async fn check_balances(&self) -> Vec<BalanceCheckResult> {
        let mut results = Vec::with_capacity(self.wallet_pubkeys.len());

        for pubkey in &self.wallet_pubkeys {
            match self.fetch_balance(pubkey).await {
                Ok(lamports) => {
                    let balance_sol = lamports as f64 / LAMPORTS_PER_SOL as f64;
                    gauge!("solvela_fee_payer_balance_sol", "pubkey" => pubkey.clone())
                        .set(balance_sol);
                    let alert_level = self.alert_level(balance_sol);
                    results.push(BalanceCheckResult {
                        wallet_pubkey: pubkey.clone(),
                        balance_sol,
                        lamports,
                        alert_level,
                    });
                }
                Err(e) => {
                    warn!(
                        wallet = %pubkey,
                        error = %e,
                        "failed to fetch SOL balance — skipping wallet"
                    );
                }
            }
        }

        results
    }

    /// Emit structured tracing events based on balance check results.
    pub fn emit_alerts(&self, results: &[BalanceCheckResult]) {
        for result in results {
            match result.alert_level {
                AlertLevel::Critical => {
                    error!(
                        wallet = %result.wallet_pubkey,
                        balance_sol = %result.balance_sol,
                        lamports = result.lamports,
                        threshold_sol = %self.config.critical_threshold_sol,
                        "CRITICAL: fee-payer wallet SOL balance is critically low"
                    );
                }
                AlertLevel::Warning => {
                    warn!(
                        wallet = %result.wallet_pubkey,
                        balance_sol = %result.balance_sol,
                        lamports = result.lamports,
                        threshold_sol = %self.config.warn_threshold_sol,
                        "fee-payer wallet SOL balance is low"
                    );
                }
                AlertLevel::Healthy => {
                    info!(
                        wallet = %result.wallet_pubkey,
                        balance_sol = %result.balance_sol,
                        lamports = result.lamports,
                        "fee-payer wallet balance OK"
                    );
                }
            }
        }
    }

    /// Spawn a background task that periodically checks balances and emits alerts.
    ///
    /// Runs an immediate check on startup, then loops at `check_interval_secs`.
    /// Shuts down gracefully when the `shutdown_rx` watch channel fires.
    /// The returned `JoinHandle` is fire-and-forget — callers should not `.await` it
    /// on the hot path.
    pub fn spawn(
        self: Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_secs(self.config.check_interval_secs);

        tokio::spawn(async move {
            // Immediate first check so operators see balance status on startup
            let results = self.check_balances().await;
            self.emit_alerts(&results);

            let mut ticker = tokio::time::interval(interval);
            // The first tick fires immediately — skip it since we already checked
            ticker.tick().await;

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let results = self.check_balances().await;
                        self.emit_alerts(&results);
                    }
                    _ = shutdown_rx.changed() => {
                        info!("balance monitor shutting down gracefully");
                        break;
                    }
                }
            }
        })
    }

    /// Fetch the SOL balance (in lamports) for a single wallet via JSON-RPC.
    async fn fetch_balance(&self, pubkey: &str) -> Result<u64, BalanceMonitorError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [pubkey]
        });

        let response = self
            .http_client
            .post(&self.config.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| BalanceMonitorError::Rpc(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(BalanceMonitorError::Rpc(format!(
                "RPC returned HTTP {status}"
            )));
        }

        let rpc_response: RpcBalanceResponse = response
            .json()
            .await
            .map_err(|e| BalanceMonitorError::Parse(e.to_string()))?;

        if let Some(err) = rpc_response.error {
            return Err(BalanceMonitorError::Rpc(format!(
                "RPC error {}: {}",
                err.code, err.message
            )));
        }

        rpc_response.result.map(|r| r.value).ok_or_else(|| {
            BalanceMonitorError::Parse("missing result field in RPC response".into())
        })
    }
}

/// Errors that can occur during balance monitoring.
#[derive(Debug, thiserror::Error)]
pub enum BalanceMonitorError {
    #[error("RPC request failed: {0}")]
    Rpc(String),
    #[error("failed to parse RPC response: {0}")]
    Parse(String),
}

/// JSON-RPC response for `getBalance`.
#[derive(Debug, Deserialize)]
struct RpcBalanceResponse {
    result: Option<RpcBalanceResult>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcBalanceResult {
    value: u64,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // AlertLevel classification (requires BalanceMonitor::alert_level)
    // -------------------------------------------------------------------------

    fn make_monitor(warn: f64, critical: f64) -> BalanceMonitor {
        let config = MonitorConfig {
            warn_threshold_sol: warn,
            critical_threshold_sol: critical,
            check_interval_secs: 300,
            rpc_url: "https://api.devnet.solana.com".to_string(),
        };
        BalanceMonitor::new(config, vec![])
    }

    #[test]
    fn test_alert_level_healthy() {
        let monitor = make_monitor(0.1, 0.02);
        assert_eq!(monitor.alert_level(1.0), AlertLevel::Healthy);
    }

    #[test]
    fn test_alert_level_warning() {
        let monitor = make_monitor(0.1, 0.02);
        // 0.05 SOL is below warn (0.1) but above critical (0.02)
        assert_eq!(monitor.alert_level(0.05), AlertLevel::Warning);
    }

    #[test]
    fn test_alert_level_critical() {
        let monitor = make_monitor(0.1, 0.02);
        // 0.01 SOL is below critical (0.02)
        assert_eq!(monitor.alert_level(0.01), AlertLevel::Critical);
    }

    #[test]
    fn test_alert_level_boundary_warn() {
        let monitor = make_monitor(0.1, 0.02);
        // Exactly at warn threshold → Healthy (threshold is exclusive: < warn triggers warning)
        assert_eq!(monitor.alert_level(0.1), AlertLevel::Healthy);
    }

    #[test]
    fn test_alert_level_boundary_critical() {
        let monitor = make_monitor(0.1, 0.02);
        // Exactly at critical threshold → Warning (threshold is exclusive: < critical triggers critical)
        assert_eq!(monitor.alert_level(0.02), AlertLevel::Warning);
    }

    // -------------------------------------------------------------------------
    // MonitorConfig defaults
    // -------------------------------------------------------------------------

    #[test]
    fn test_monitor_config_defaults() {
        let config = MonitorConfig::default();
        assert!((config.warn_threshold_sol - 0.1).abs() < f64::EPSILON);
        assert!((config.critical_threshold_sol - 0.02).abs() < f64::EPSILON);
        assert_eq!(config.check_interval_secs, 300);
        assert_eq!(config.rpc_url, "https://api.devnet.solana.com");
    }

    // -------------------------------------------------------------------------
    // BalanceCheckResult fields
    // -------------------------------------------------------------------------

    #[test]
    fn test_balance_check_result_fields() {
        let result = BalanceCheckResult {
            wallet_pubkey: "So11111111111111111111111111111111111111112".to_string(),
            balance_sol: 0.5,
            lamports: 500_000_000,
            alert_level: AlertLevel::Healthy,
        };
        assert_eq!(
            result.wallet_pubkey,
            "So11111111111111111111111111111111111111112"
        );
        assert!((result.balance_sol - 0.5).abs() < f64::EPSILON);
        assert_eq!(result.lamports, 500_000_000);
        assert_eq!(result.alert_level, AlertLevel::Healthy);
    }

    // -------------------------------------------------------------------------
    // Lamport → SOL formatting helper
    // -------------------------------------------------------------------------

    #[test]
    fn test_format_sol_balance() {
        assert_eq!(format_sol_balance(1_000_000_000), "1.000000000 SOL");
        assert_eq!(format_sol_balance(0), "0.000000000 SOL");
        assert_eq!(format_sol_balance(500_000_000), "0.500000000 SOL");
        assert_eq!(format_sol_balance(1), "0.000000001 SOL");
        assert_eq!(format_sol_balance(100_000_000), "0.100000000 SOL");
        assert_eq!(format_sol_balance(20_000_000), "0.020000000 SOL");
    }
}
