//! SOL + USDC balance monitoring for gateway-operated wallets.
//!
//! Periodically polls Solana RPC for the SOL balance (for tx fees) and the
//! USDC balance (for paying upstream providers) of every wallet the operator
//! cares about: the gateway recipient, the fee-payer pool, and any operator
//! wallets passed via `SOLVELA_OPERATOR_WALLETS`. Emits structured tracing
//! events + Prometheus gauges + an `AlertSink` callback on threshold
//! transitions.
//!
//! `AlertSink` is intentionally a trait so the OSS gateway ships a
//! [`NoopSink`] default and the enterprise distribution can plug in
//! Slack/email/dashboard sinks without forking. Sinks are only invoked on
//! transitions between alert levels — a wallet sitting at Critical for hours
//! fires once, not every check interval.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use metrics::gauge;
use serde::Deserialize;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Lamports per SOL (1 SOL = 1_000_000_000 lamports).
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;

/// Atomic units per USDC (USDC-SPL has 6 decimals).
const ATOMIC_UNITS_PER_USDC: u64 = 1_000_000;

/// Mainnet USDC-SPL mint, used as the monitor default. The gateway's
/// actual USDC mint is sourced from `SolanaConfig::usdc_mint` and threaded
/// through `MonitorConfig::usdc_mint` at startup. Sourced from
/// `solvela_protocol::constants` so the address is defined exactly once
/// across the workspace.
const DEFAULT_USDC_MINT: &str = solvela_protocol::constants::USDC_MINT;

/// Maximum bytes accepted from a single Solana JSON-RPC response. A real
/// `getBalance` reply is < 200 bytes and a `getTokenAccountsByOwner` reply
/// for a USDC owner with one ATA is < 2 KiB. A 256 KiB cap defends against
/// a malicious or MITM'd RPC node returning a multi-megabyte payload that
/// could OOM the gateway during deserialization, while still leaving 100×
/// headroom for any plausible real response.
const MAX_RPC_RESPONSE_BYTES: usize = 256 * 1024;

/// Default consecutive-failure count after which a wallet is reported as
/// stale (synthetic Critical event). At the default 5-minute check
/// interval, 3 failures means "no fresh data for ~15 minutes" — long
/// enough that we're confident the RPC isn't just blipping, short enough
/// that operators learn about it before the wallet actually drains
/// without anyone noticing.
const DEFAULT_STALE_AFTER_FAILURES: u32 = 3;

/// Asset class polled by the monitor. SOL pays for transaction fees; USDC is
/// what the gateway actually spends on upstream LLM providers and collects as
/// revenue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Asset {
    Sol,
    Usdc,
}

impl Asset {
    /// Lowercase label suitable for log fields and metric labels.
    pub fn label(&self) -> &'static str {
        match self {
            Asset::Sol => "sol",
            Asset::Usdc => "usdc",
        }
    }
}

/// Alert severity based on a wallet balance relative to per-asset thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AlertLevel {
    Healthy,
    Warning,
    Critical,
}

/// Result of checking a single wallet's balance for a single asset.
#[derive(Debug, Clone)]
pub struct BalanceCheckResult {
    pub wallet_pubkey: String,
    pub asset: Asset,
    /// Human-scaled balance (SOL or USDC, both as a float with their native
    /// decimal scale applied). Use [`Self::atomic_units`] for exact equality.
    pub balance_human: f64,
    /// Atomic units (lamports for SOL, 6-decimal atomic units for USDC).
    pub atomic_units: u64,
    pub alert_level: AlertLevel,
}

/// Event passed to [`AlertSink::dispatch`] when a wallet's alert level
/// transitions. `previous_level` is `None` on the first observation for a
/// given (wallet, asset) pair so sinks can distinguish "wallet came up
/// already low" from "wallet just dropped".
///
/// When `is_stale` is set, `balance_human` and `atomic_units` reflect the
/// *last known good* observation (or 0 if there has never been one) and
/// the event was synthesized because RPC has been failing for
/// `MonitorConfig::stale_after_failures` consecutive checks. Without this
/// signal, sustained RPC outage would mean a wallet that actually drains
/// to zero during the outage never triggers a sink dispatch.
#[derive(Debug, Clone)]
pub struct BalanceAlertEvent {
    pub wallet_pubkey: String,
    pub asset: Asset,
    pub balance_human: f64,
    pub atomic_units: u64,
    pub previous_level: Option<AlertLevel>,
    pub current_level: AlertLevel,
    /// True when this event was synthesized due to repeated RPC failures
    /// rather than an observed balance crossing a threshold.
    pub is_stale: bool,
}

/// Sink for alert-level transitions.
///
/// The OSS gateway ships [`NoopSink`] as the default — operators get logs
/// and the Prometheus gauge for free, and external Alertmanager rules can
/// page on those. Enterprise distributions plug in their own sink
/// (Slack/email/dashboard) without touching this crate.
#[async_trait]
pub trait AlertSink: Send + Sync {
    async fn dispatch(&self, event: &BalanceAlertEvent);
}

/// No-op default sink used when no enterprise sink is wired in.
pub struct NoopSink;

#[async_trait]
impl AlertSink for NoopSink {
    async fn dispatch(&self, _event: &BalanceAlertEvent) {}
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
    /// USDC balance (in whole USDC) below which a warning is emitted.
    #[serde(default = "default_warn_threshold_usdc")]
    pub warn_threshold_usdc: f64,
    /// USDC balance (in whole USDC) below which a critical error is emitted.
    #[serde(default = "default_critical_threshold_usdc")]
    pub critical_threshold_usdc: f64,
    /// Seconds between balance checks.
    #[serde(default = "default_check_interval")]
    pub check_interval_secs: u64,
    /// Solana RPC URL used for `getBalance` + `getTokenAccountsByOwner`.
    #[serde(default = "default_rpc_url")]
    pub rpc_url: String,
    /// USDC-SPL mint to poll for token balances. Threaded from
    /// `SolanaConfig::usdc_mint` at startup; the default here is the mainnet
    /// mint so a config-less test still has a sane value.
    #[serde(default = "default_usdc_mint")]
    pub usdc_mint: String,
    /// Number of consecutive RPC failures (per wallet, per asset) after
    /// which the monitor emits a synthetic Critical event with
    /// `is_stale = true`. Defends against the failure mode where RPC is
    /// down for hours and a wallet silently drains without alerts.
    #[serde(default = "default_stale_after_failures")]
    pub stale_after_failures: u32,
}

fn default_warn_threshold() -> f64 {
    0.1
}

fn default_critical_threshold() -> f64 {
    0.02
}

fn default_warn_threshold_usdc() -> f64 {
    // Tuned for a production gateway that spends USDC continuously on
    // upstream LLM calls. At ~$5/min steady-state spend on a small
    // gateway, $50 warn gives the operator ~10 minutes to top up before
    // the critical threshold trips. Operators running at different scales
    // should override via SOLVELA_MONITOR__WARN_THRESHOLD_USDC.
    50.0
}

fn default_critical_threshold_usdc() -> f64 {
    5.0
}

fn default_check_interval() -> u64 {
    300
}

fn default_rpc_url() -> String {
    "https://api.devnet.solana.com".to_string()
}

fn default_usdc_mint() -> String {
    DEFAULT_USDC_MINT.to_string()
}

fn default_stale_after_failures() -> u32 {
    DEFAULT_STALE_AFTER_FAILURES
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            warn_threshold_sol: default_warn_threshold(),
            critical_threshold_sol: default_critical_threshold(),
            warn_threshold_usdc: default_warn_threshold_usdc(),
            critical_threshold_usdc: default_critical_threshold_usdc(),
            check_interval_secs: default_check_interval(),
            rpc_url: default_rpc_url(),
            usdc_mint: default_usdc_mint(),
            stale_after_failures: default_stale_after_failures(),
        }
    }
}

/// Format a lamport balance as a human-readable SOL string with 9 decimal places.
pub fn format_sol_balance(lamports: u64) -> String {
    let sol = lamports / LAMPORTS_PER_SOL;
    let remainder = lamports % LAMPORTS_PER_SOL;
    format!("{sol}.{remainder:09} SOL")
}

/// Format a USDC atomic-unit balance (6 decimals) as a human-readable string.
pub fn format_usdc_balance(atomic_units: u64) -> String {
    let usdc = atomic_units / ATOMIC_UNITS_PER_USDC;
    let remainder = atomic_units % ATOMIC_UNITS_PER_USDC;
    format!("{usdc}.{remainder:06} USDC")
}

/// Last-known-good observation for a (wallet, asset) pair. Reflected in
/// synthetic stale events so operators can still see what value preceded
/// the RPC outage. `None` for [`Self::last_balance`] means the monitor
/// has never observed a successful balance for this pair.
#[derive(Debug, Clone, Default)]
struct WalletState {
    /// Last observed alert level. Drives transition dedup.
    last_level: Option<AlertLevel>,
    /// Last observed atomic_units (lamports for SOL, USDC 6-decimal units).
    last_atomic_units: Option<u64>,
    /// Last observed human-scaled balance.
    last_balance_human: Option<f64>,
    /// Consecutive RPC fetch failures since the last success.
    consecutive_failures: u32,
    /// True once a synthetic stale event has been emitted at the current
    /// failure streak. Reset on the next successful fetch.
    stale_event_emitted: bool,
}

/// Background balance monitor that periodically polls SOL + USDC balances
/// of configured wallets via the Solana JSON-RPC.
pub struct BalanceMonitor {
    config: MonitorConfig,
    wallet_pubkeys: Vec<String>,
    http_client: reqwest::Client,
    sink: Arc<dyn AlertSink>,
    /// Per-(wallet, asset) tracking state — last-seen alert level (drives
    /// transition dedup), last-known-good balance (for stale events), and
    /// consecutive RPC failure count (drives stale detection).
    states: Mutex<HashMap<(String, Asset), WalletState>>,
}

impl BalanceMonitor {
    /// Create a new monitor with the default [`NoopSink`].
    ///
    /// Returns `Err(reqwest::Error)` if the HTTP client fails to build —
    /// previously we silently fell back to `Client::default()` (no
    /// timeout), which under unusual TLS-init conditions could leave
    /// `fetch_balance` hanging indefinitely and block graceful shutdown.
    /// Failing startup is the correct response: no balance monitoring is
    /// better than a stuck-forever monitoring task.
    pub fn new(config: MonitorConfig, wallet_pubkeys: Vec<String>) -> Result<Self, reqwest::Error> {
        Self::with_sink(config, wallet_pubkeys, Arc::new(NoopSink))
    }

    /// Create a new monitor with a custom [`AlertSink`].
    ///
    /// This is the entry point enterprise distributions use to plug in their
    /// own delivery channels (Slack/email/dashboard) without forking. The
    /// OSS path through [`Self::new`] uses [`NoopSink`].
    pub fn with_sink(
        config: MonitorConfig,
        wallet_pubkeys: Vec<String>,
        sink: Arc<dyn AlertSink>,
    ) -> Result<Self, reqwest::Error> {
        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()?;

        Ok(Self {
            config,
            wallet_pubkeys,
            http_client,
            sink,
            states: Mutex::new(HashMap::new()),
        })
    }

    /// Returns the number of wallets being monitored.
    pub fn wallet_count(&self) -> usize {
        self.wallet_pubkeys.len()
    }

    /// Determine alert level for the given asset balance.
    ///
    /// Thresholds are exclusive:
    /// - `balance < critical` → `Critical`
    /// - `balance < warn` → `Warning`
    /// - otherwise → `Healthy`
    pub fn alert_level_for(&self, asset: Asset, balance_human: f64) -> AlertLevel {
        let (warn, crit) = match asset {
            Asset::Sol => (
                self.config.warn_threshold_sol,
                self.config.critical_threshold_sol,
            ),
            Asset::Usdc => (
                self.config.warn_threshold_usdc,
                self.config.critical_threshold_usdc,
            ),
        };
        if balance_human < crit {
            AlertLevel::Critical
        } else if balance_human < warn {
            AlertLevel::Warning
        } else {
            AlertLevel::Healthy
        }
    }

    /// Backward-compatible SOL alert level — kept so external callers and
    /// existing tests that only know about SOL don't need to thread an
    /// `Asset` argument.
    pub fn alert_level(&self, balance_sol: f64) -> AlertLevel {
        self.alert_level_for(Asset::Sol, balance_sol)
    }

    /// Check SOL + USDC balances for all configured wallets via Solana RPC.
    ///
    /// On RPC error for a specific (wallet, asset) tuple, logs a warning
    /// AND increments the consecutive-failure counter for that pair. Once
    /// the counter reaches [`MonitorConfig::stale_after_failures`], the
    /// next call to [`Self::dispatch_transitions`] will emit a synthetic
    /// Critical event with `is_stale = true`. This is the key defense
    /// against the failure mode where RPC is down for hours and a wallet
    /// silently drains: without it, no transition would fire and the
    /// sink would stay quiet despite a real outage. Never panics.
    pub async fn check_balances(&self) -> Vec<BalanceCheckResult> {
        let mut results = Vec::with_capacity(self.wallet_pubkeys.len() * 2);

        for pubkey in &self.wallet_pubkeys {
            // ── SOL ──
            match self.fetch_sol_balance(pubkey).await {
                Ok(lamports) => {
                    let balance_sol = lamports as f64 / LAMPORTS_PER_SOL as f64;
                    gauge!("solvela_wallet_balance_sol", "pubkey" => pubkey.clone())
                        .set(balance_sol);
                    // Keep the legacy gauge name so existing dashboards /
                    // Alertmanager rules built against `solvela_fee_payer_balance_sol`
                    // don't silently break. TODO(deprecation 2026-Q4): drop
                    // once dashboards are migrated to `solvela_wallet_balance_sol`.
                    gauge!("solvela_fee_payer_balance_sol", "pubkey" => pubkey.clone())
                        .set(balance_sol);
                    let alert_level = self.alert_level_for(Asset::Sol, balance_sol);
                    self.record_success(pubkey, Asset::Sol, lamports, balance_sol)
                        .await;
                    results.push(BalanceCheckResult {
                        wallet_pubkey: pubkey.clone(),
                        asset: Asset::Sol,
                        balance_human: balance_sol,
                        atomic_units: lamports,
                        alert_level,
                    });
                }
                Err(e) => {
                    warn!(
                        wallet = %pubkey,
                        asset = Asset::Sol.label(),
                        error = %e,
                        "failed to fetch SOL balance — skipping wallet"
                    );
                    self.record_failure(pubkey, Asset::Sol).await;
                }
            }

            // ── USDC ──
            if self.config.usdc_mint.is_empty() {
                // Surface the misconfig at debug level (not warn — `info!`
                // at startup already announces it). Without this branch
                // an operator who clears the mint env var would see USDC
                // silently disappear from every check.
                debug!(
                    wallet = %pubkey,
                    "usdc_mint is empty — skipping USDC balance check"
                );
                continue;
            }
            match self.fetch_usdc_balance(pubkey).await {
                Ok(atomic) => {
                    let balance_usdc = atomic as f64 / ATOMIC_UNITS_PER_USDC as f64;
                    gauge!("solvela_wallet_balance_usdc", "pubkey" => pubkey.clone())
                        .set(balance_usdc);
                    let alert_level = self.alert_level_for(Asset::Usdc, balance_usdc);
                    self.record_success(pubkey, Asset::Usdc, atomic, balance_usdc)
                        .await;
                    results.push(BalanceCheckResult {
                        wallet_pubkey: pubkey.clone(),
                        asset: Asset::Usdc,
                        balance_human: balance_usdc,
                        atomic_units: atomic,
                        alert_level,
                    });
                }
                Err(e) => {
                    warn!(
                        wallet = %pubkey,
                        asset = Asset::Usdc.label(),
                        error = %e,
                        "failed to fetch USDC balance — skipping wallet"
                    );
                    self.record_failure(pubkey, Asset::Usdc).await;
                }
            }
        }

        results
    }

    /// Update the per-(wallet, asset) state on a successful fetch: reset
    /// the failure counter, clear the stale flag, and record the new
    /// last-known-good values.
    async fn record_success(&self, pubkey: &str, asset: Asset, atomic: u64, human: f64) {
        let mut states = self.states.lock().await;
        let state = states
            .entry((pubkey.to_string(), asset))
            .or_insert_with(WalletState::default);
        state.consecutive_failures = 0;
        state.stale_event_emitted = false;
        state.last_atomic_units = Some(atomic);
        state.last_balance_human = Some(human);
    }

    /// Increment the consecutive-failure counter for a (wallet, asset)
    /// pair. The stale event itself is emitted by
    /// [`Self::dispatch_transitions`] so all sink dispatch goes through
    /// one code path.
    async fn record_failure(&self, pubkey: &str, asset: Asset) {
        let mut states = self.states.lock().await;
        let state = states
            .entry((pubkey.to_string(), asset))
            .or_insert_with(WalletState::default);
        state.consecutive_failures = state.consecutive_failures.saturating_add(1);
    }

    /// Emit structured tracing events for every balance check. Always logs
    /// (so operators can correlate against the gauge); transitions are
    /// dispatched separately via [`Self::dispatch_transitions`].
    pub fn emit_alerts(&self, results: &[BalanceCheckResult]) {
        for result in results {
            let threshold = self.threshold_for(result.asset, result.alert_level);
            match result.alert_level {
                AlertLevel::Critical => {
                    error!(
                        wallet = %result.wallet_pubkey,
                        asset = result.asset.label(),
                        balance = %result.balance_human,
                        atomic_units = result.atomic_units,
                        threshold = %threshold,
                        "CRITICAL: wallet balance is critically low"
                    );
                }
                AlertLevel::Warning => {
                    warn!(
                        wallet = %result.wallet_pubkey,
                        asset = result.asset.label(),
                        balance = %result.balance_human,
                        atomic_units = result.atomic_units,
                        threshold = %threshold,
                        "wallet balance is low"
                    );
                }
                AlertLevel::Healthy => {
                    // `debug!` (not `info!`): with a 5-minute check interval
                    // and N wallets × 2 assets, an INFO line per check
                    // streams the entire wallet roster to log aggregators /
                    // SIEMs forever. Solana pubkeys aren't secrets in
                    // isolation, but the operator's full wallet composition
                    // is sensitive infra info. Warning/Critical stay at
                    // their current levels — operators need those.
                    debug!(
                        wallet = %result.wallet_pubkey,
                        asset = result.asset.label(),
                        balance = %result.balance_human,
                        atomic_units = result.atomic_units,
                        "wallet balance OK"
                    );
                }
            }
        }
    }

    /// Dispatch alert-sink events for level transitions and for
    /// stale-data conditions. Returns the events that fired so
    /// callers/tests can inspect them.
    ///
    /// Three kinds of events are produced:
    ///
    /// 1. **Level transition** — a (wallet, asset) crosses a threshold.
    ///    `is_stale = false`.
    /// 2. **First observation** — first time we see a (wallet, asset).
    ///    `previous_level = None`, `is_stale = false`.
    /// 3. **Stale data** — RPC has failed `stale_after_failures` times in
    ///    a row for a (wallet, asset). `current_level = Critical`,
    ///    `is_stale = true`, balance fields reflect last known good (or
    ///    0 if never observed). Fires once per failure streak, then
    ///    resets when RPC recovers.
    ///
    /// Concurrency invariant: `states` is updated optimistically *before*
    /// the sink is invoked, and the mutex is released before any `.await`
    /// on the sink. This is deliberate:
    ///
    /// 1. We MUST NOT hold the lock across `sink.dispatch(...).await`. A
    ///    slow enterprise sink (HTTP to Slack/PagerDuty) could otherwise
    ///    block every other caller of `dispatch_transitions`, and a sink
    ///    that recursively touched the monitor would deadlock.
    /// 2. Updating state before dispatch means a second concurrent check
    ///    observing the same transition will NOT re-fire it — even if
    ///    the first sink call is still in flight. Sinks are expected to
    ///    be best-effort (logs + metrics already cover durable storage).
    pub async fn dispatch_transitions(
        &self,
        results: &[BalanceCheckResult],
    ) -> Vec<BalanceAlertEvent> {
        let pending: Vec<BalanceAlertEvent> = {
            let mut states = self.states.lock().await;
            let mut pending = Vec::new();

            // First: transitions on successfully-fetched results.
            for result in results {
                let key = (result.wallet_pubkey.clone(), result.asset);
                let state = states
                    .entry(key.clone())
                    .or_insert_with(WalletState::default);
                let previous = state.last_level;
                let changed = match previous {
                    Some(prev) => prev != result.alert_level,
                    None => true,
                };
                if changed {
                    pending.push(BalanceAlertEvent {
                        wallet_pubkey: result.wallet_pubkey.clone(),
                        asset: result.asset,
                        balance_human: result.balance_human,
                        atomic_units: result.atomic_units,
                        previous_level: previous,
                        current_level: result.alert_level,
                        is_stale: false,
                    });
                    state.last_level = Some(result.alert_level);
                } else {
                    debug!(
                        wallet = %result.wallet_pubkey,
                        asset = result.asset.label(),
                        level = ?result.alert_level,
                        "balance level unchanged — skipping sink dispatch"
                    );
                }
            }

            // Second: stale events for (wallet, asset) pairs whose failure
            // count has crossed the threshold AND that haven't already
            // fired a stale event this streak.
            let threshold = self.config.stale_after_failures;
            for ((pubkey, asset), state) in states.iter_mut() {
                if state.consecutive_failures >= threshold && !state.stale_event_emitted {
                    let atomic = state.last_atomic_units.unwrap_or(0);
                    let human = state.last_balance_human.unwrap_or(0.0);
                    pending.push(BalanceAlertEvent {
                        wallet_pubkey: pubkey.clone(),
                        asset: *asset,
                        balance_human: human,
                        atomic_units: atomic,
                        previous_level: state.last_level,
                        current_level: AlertLevel::Critical,
                        is_stale: true,
                    });
                    state.stale_event_emitted = true;
                    error!(
                        wallet = %pubkey,
                        asset = asset.label(),
                        consecutive_failures = state.consecutive_failures,
                        threshold = threshold,
                        "CRITICAL: RPC has failed repeatedly — balance is stale, cannot confirm wallet is funded"
                    );
                }
            }

            pending
        };

        // Phase 2: dispatch outside the lock. A slow or recursive sink can
        // no longer block other callers or deadlock the monitor.
        for event in &pending {
            self.sink.dispatch(event).await;
        }
        pending
    }

    /// Threshold value for the given (asset, level) pair. Returns 0.0 for
    /// `Healthy` (no threshold crossed); used to enrich log lines.
    fn threshold_for(&self, asset: Asset, level: AlertLevel) -> f64 {
        match (asset, level) {
            (Asset::Sol, AlertLevel::Warning) => self.config.warn_threshold_sol,
            (Asset::Sol, AlertLevel::Critical) => self.config.critical_threshold_sol,
            (Asset::Usdc, AlertLevel::Warning) => self.config.warn_threshold_usdc,
            (Asset::Usdc, AlertLevel::Critical) => self.config.critical_threshold_usdc,
            (_, AlertLevel::Healthy) => 0.0,
        }
    }

    /// Spawn a background task that periodically checks balances, emits
    /// log/metric alerts, and dispatches sink events on transitions.
    ///
    /// Runs an immediate check on startup, then loops at `check_interval_secs`.
    /// Shuts down gracefully when the `shutdown_rx` watch channel fires.
    /// The returned `JoinHandle` is fire-and-forget — callers should not
    /// `.await` it on the hot path.
    pub fn spawn(
        self: Arc<Self>,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let interval = Duration::from_secs(self.config.check_interval_secs);

        tokio::spawn(async move {
            // Immediate first check so operators see balance status on startup
            let results = self.check_balances().await;
            self.emit_alerts(&results);
            self.dispatch_transitions(&results).await;

            let mut ticker = tokio::time::interval(interval);
            // The first tick fires immediately — skip it since we already checked
            ticker.tick().await;

            loop {
                tokio::select! {
                    _ = ticker.tick() => {
                        let results = self.check_balances().await;
                        self.emit_alerts(&results);
                        self.dispatch_transitions(&results).await;
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
    async fn fetch_sol_balance(&self, pubkey: &str) -> Result<u64, BalanceMonitorError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getBalance",
            "params": [pubkey]
        });

        let bytes = self.send_rpc(&body).await?;
        let rpc_response: RpcBalanceResponse = serde_json::from_slice(&bytes)
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

    /// Fetch the USDC balance (in atomic units, 6 decimals) for a single
    /// wallet via `getTokenAccountsByOwner`. Returns 0 when the wallet has
    /// no associated token account for the configured mint (an unfunded
    /// ATA is the on-chain equivalent of zero, not an error). Sums across
    /// multiple ATAs for the same mint with saturating addition so a
    /// pathologically constructed account list can't underflow downstream
    /// arithmetic.
    ///
    /// Trust boundary: the response body is hard-capped at
    /// `MAX_RPC_RESPONSE_BYTES` so a compromised RPC endpoint can't OOM
    /// the gateway by returning a massive token-account list. Saturating
    /// addition still means a malicious RPC can mask a low balance by
    /// returning `u64::MAX` atomic units; operators relying on this
    /// monitor for critical alerting must use a trusted/authenticated
    /// RPC endpoint.
    async fn fetch_usdc_balance(&self, owner: &str) -> Result<u64, BalanceMonitorError> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getTokenAccountsByOwner",
            "params": [
                owner,
                { "mint": self.config.usdc_mint },
                { "encoding": "jsonParsed" }
            ]
        });

        let bytes = self.send_rpc(&body).await?;
        let rpc_response: RpcTokenAccountsResponse = serde_json::from_slice(&bytes)
            .map_err(|e| BalanceMonitorError::Parse(e.to_string()))?;

        if let Some(err) = rpc_response.error {
            return Err(BalanceMonitorError::Rpc(format!(
                "RPC error {}: {}",
                err.code, err.message
            )));
        }

        let result = rpc_response.result.ok_or_else(|| {
            BalanceMonitorError::Parse("missing result field in RPC response".into())
        })?;

        parse_usdc_total(&result.value)
    }

    /// POST a JSON-RPC body and return the raw response bytes, capped at
    /// [`MAX_RPC_RESPONSE_BYTES`]. Centralizing the byte cap here means
    /// every future RPC method we add inherits the OOM defense.
    async fn send_rpc(&self, body: &serde_json::Value) -> Result<Vec<u8>, BalanceMonitorError> {
        let response = self
            .http_client
            .post(&self.config.rpc_url)
            .json(body)
            .send()
            .await
            .map_err(|e| BalanceMonitorError::Rpc(e.to_string()))?;

        let status = response.status();
        if !status.is_success() {
            return Err(BalanceMonitorError::Rpc(format!(
                "RPC returned HTTP {status}"
            )));
        }

        // Enforce a hard byte cap. `Content-Length` is advisory; we
        // additionally accumulate chunks so a server that lies about
        // length (or omits it under chunked transfer encoding) still
        // can't push us over the cap.
        if let Some(len) = response.content_length() {
            if len as usize > MAX_RPC_RESPONSE_BYTES {
                return Err(BalanceMonitorError::Rpc(format!(
                    "RPC response exceeds cap ({len} > {MAX_RPC_RESPONSE_BYTES} bytes)"
                )));
            }
        }

        let mut buf = Vec::with_capacity(2048);
        let mut response = response;
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| BalanceMonitorError::Rpc(e.to_string()))?
        {
            if buf.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
                return Err(BalanceMonitorError::Rpc(format!(
                    "RPC response exceeded cap of {MAX_RPC_RESPONSE_BYTES} bytes during streaming"
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        Ok(buf)
    }
}

/// Sum the `amount` field across all token accounts in a
/// `getTokenAccountsByOwner` response, with saturating addition.
fn parse_usdc_total(accounts: &[RpcTokenAccount]) -> Result<u64, BalanceMonitorError> {
    let mut total: u64 = 0;
    for acct in accounts {
        let amount_str = &acct.account.data.parsed.info.token_amount.amount;
        let amount: u64 = amount_str.parse().map_err(|e| {
            BalanceMonitorError::Parse(format!("invalid token amount '{amount_str}': {e}"))
        })?;
        total = total.saturating_add(amount);
    }
    Ok(total)
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

/// JSON-RPC response for `getTokenAccountsByOwner` with `encoding=jsonParsed`.
#[derive(Debug, Deserialize)]
struct RpcTokenAccountsResponse {
    result: Option<RpcTokenAccountsResult>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccountsResult {
    #[serde(default)]
    value: Vec<RpcTokenAccount>,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccount {
    account: RpcTokenAccountInner,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccountInner {
    data: RpcTokenAccountData,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccountData {
    parsed: RpcTokenAccountParsed,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccountParsed {
    info: RpcTokenAccountInfo,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAccountInfo {
    #[serde(rename = "tokenAmount")]
    token_amount: RpcTokenAmount,
}

#[derive(Debug, Deserialize)]
struct RpcTokenAmount {
    /// Atomic units as a base-10 string (the RPC always returns this as a
    /// string to avoid JSON-number precision loss on large supplies).
    amount: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    // -------------------------------------------------------------------------
    // AlertLevel classification — SOL
    // -------------------------------------------------------------------------

    fn make_monitor_sol(warn: f64, critical: f64) -> BalanceMonitor {
        let config = MonitorConfig {
            warn_threshold_sol: warn,
            critical_threshold_sol: critical,
            ..MonitorConfig::default()
        };
        BalanceMonitor::new(config, vec![]).expect("test client builds")
    }

    fn make_monitor_usdc(warn: f64, critical: f64) -> BalanceMonitor {
        let config = MonitorConfig {
            warn_threshold_usdc: warn,
            critical_threshold_usdc: critical,
            ..MonitorConfig::default()
        };
        BalanceMonitor::new(config, vec![]).expect("test client builds")
    }

    /// Builder for tests that need to drive the per-(wallet, asset) state
    /// directly — used by stale-data tests below.
    fn make_monitor_stale(stale_after: u32, sink: Arc<dyn AlertSink>) -> BalanceMonitor {
        let config = MonitorConfig {
            stale_after_failures: stale_after,
            ..MonitorConfig::default()
        };
        BalanceMonitor::with_sink(config, vec!["w1".to_string()], sink).expect("test client builds")
    }

    #[test]
    fn test_alert_level_healthy() {
        let monitor = make_monitor_sol(0.1, 0.02);
        assert_eq!(monitor.alert_level(1.0), AlertLevel::Healthy);
    }

    #[test]
    fn test_alert_level_warning() {
        let monitor = make_monitor_sol(0.1, 0.02);
        assert_eq!(monitor.alert_level(0.05), AlertLevel::Warning);
    }

    #[test]
    fn test_alert_level_critical() {
        let monitor = make_monitor_sol(0.1, 0.02);
        assert_eq!(monitor.alert_level(0.01), AlertLevel::Critical);
    }

    #[test]
    fn test_alert_level_boundary_warn() {
        let monitor = make_monitor_sol(0.1, 0.02);
        // Exactly at warn threshold → Healthy (threshold is exclusive: < warn triggers warning)
        assert_eq!(monitor.alert_level(0.1), AlertLevel::Healthy);
    }

    #[test]
    fn test_alert_level_boundary_critical() {
        let monitor = make_monitor_sol(0.1, 0.02);
        // Exactly at critical threshold → Warning (threshold is exclusive: < critical triggers critical)
        assert_eq!(monitor.alert_level(0.02), AlertLevel::Warning);
    }

    // -------------------------------------------------------------------------
    // AlertLevel classification — USDC (independent thresholds)
    // -------------------------------------------------------------------------

    #[test]
    fn test_alert_level_usdc_healthy() {
        let monitor = make_monitor_usdc(5.0, 0.5);
        assert_eq!(
            monitor.alert_level_for(Asset::Usdc, 100.0),
            AlertLevel::Healthy
        );
    }

    #[test]
    fn test_alert_level_usdc_warning() {
        let monitor = make_monitor_usdc(5.0, 0.5);
        assert_eq!(
            monitor.alert_level_for(Asset::Usdc, 1.0),
            AlertLevel::Warning
        );
    }

    #[test]
    fn test_alert_level_usdc_critical() {
        let monitor = make_monitor_usdc(5.0, 0.5);
        assert_eq!(
            monitor.alert_level_for(Asset::Usdc, 0.0),
            AlertLevel::Critical
        );
    }

    #[test]
    fn test_alert_level_usdc_zero_is_critical() {
        // The headline "wallets on zero" case: literal zero must trip
        // Critical regardless of how operators tune their thresholds.
        let monitor = make_monitor_usdc(5.0, 0.5);
        assert_eq!(
            monitor.alert_level_for(Asset::Usdc, 0.0),
            AlertLevel::Critical
        );
        let sol_monitor = make_monitor_sol(0.1, 0.02);
        assert_eq!(
            sol_monitor.alert_level_for(Asset::Sol, 0.0),
            AlertLevel::Critical
        );
    }

    // -------------------------------------------------------------------------
    // MonitorConfig defaults
    // -------------------------------------------------------------------------

    #[test]
    fn test_monitor_config_defaults() {
        let config = MonitorConfig::default();
        assert!((config.warn_threshold_sol - 0.1).abs() < f64::EPSILON);
        assert!((config.critical_threshold_sol - 0.02).abs() < f64::EPSILON);
        assert!((config.warn_threshold_usdc - 50.0).abs() < f64::EPSILON);
        assert!((config.critical_threshold_usdc - 5.0).abs() < f64::EPSILON);
        assert_eq!(config.check_interval_secs, 300);
        assert_eq!(config.rpc_url, "https://api.devnet.solana.com");
        assert_eq!(config.usdc_mint, DEFAULT_USDC_MINT);
        assert_eq!(config.stale_after_failures, DEFAULT_STALE_AFTER_FAILURES);
    }

    // -------------------------------------------------------------------------
    // BalanceCheckResult fields
    // -------------------------------------------------------------------------

    #[test]
    fn test_balance_check_result_fields() {
        let result = BalanceCheckResult {
            wallet_pubkey: "So11111111111111111111111111111111111111112".to_string(),
            asset: Asset::Sol,
            balance_human: 0.5,
            atomic_units: 500_000_000,
            alert_level: AlertLevel::Healthy,
        };
        assert_eq!(
            result.wallet_pubkey,
            "So11111111111111111111111111111111111111112"
        );
        assert_eq!(result.asset, Asset::Sol);
        assert!((result.balance_human - 0.5).abs() < f64::EPSILON);
        assert_eq!(result.atomic_units, 500_000_000);
        assert_eq!(result.alert_level, AlertLevel::Healthy);
    }

    // -------------------------------------------------------------------------
    // Format helpers
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

    #[test]
    fn test_format_usdc_balance() {
        assert_eq!(format_usdc_balance(1_000_000), "1.000000 USDC");
        assert_eq!(format_usdc_balance(0), "0.000000 USDC");
        assert_eq!(format_usdc_balance(500_000), "0.500000 USDC");
        assert_eq!(format_usdc_balance(1), "0.000001 USDC");
    }

    #[test]
    fn test_asset_label() {
        assert_eq!(Asset::Sol.label(), "sol");
        assert_eq!(Asset::Usdc.label(), "usdc");
    }

    // -------------------------------------------------------------------------
    // USDC RPC response parsing — covers the hot edge cases without HTTP
    // -------------------------------------------------------------------------

    fn parse_usdc_from_json(payload: serde_json::Value) -> Result<u64, BalanceMonitorError> {
        let resp: RpcTokenAccountsResponse =
            serde_json::from_value(payload).expect("test fixture parses");
        if let Some(err) = resp.error {
            return Err(BalanceMonitorError::Rpc(format!(
                "RPC error {}: {}",
                err.code, err.message
            )));
        }
        let value = resp.result.expect("test fixture has result").value;
        parse_usdc_total(&value)
    }

    #[test]
    fn test_parse_usdc_no_token_account() {
        // Wallet has never held USDC — `value` is an empty array.
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": { "context": { "slot": 0 }, "value": [] }
        });
        assert_eq!(parse_usdc_from_json(payload).unwrap(), 0);
    }

    #[test]
    fn test_parse_usdc_single_ata() {
        // Single ATA with 5 USDC = 5_000_000 atomic units.
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": [{
                    "account": {
                        "data": {
                            "parsed": {
                                "info": {
                                    "tokenAmount": {
                                        "amount": "5000000",
                                        "decimals": 6,
                                        "uiAmount": 5.0,
                                        "uiAmountString": "5"
                                    }
                                }
                            }
                        }
                    }
                }]
            }
        });
        assert_eq!(parse_usdc_from_json(payload).unwrap(), 5_000_000);
    }

    #[test]
    fn test_parse_usdc_multiple_atas_sum() {
        // Rare but possible: multiple ATAs for the same mint owner.
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": [
                    { "account": { "data": { "parsed": { "info": { "tokenAmount": {
                        "amount": "1000000", "decimals": 6
                    }}}}}},
                    { "account": { "data": { "parsed": { "info": { "tokenAmount": {
                        "amount": "2500000", "decimals": 6
                    }}}}}}
                ]
            }
        });
        assert_eq!(parse_usdc_from_json(payload).unwrap(), 3_500_000);
    }

    #[test]
    fn test_parse_usdc_rpc_error() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32602, "message": "Invalid params" }
        });
        let err = parse_usdc_from_json(payload).unwrap_err();
        assert!(
            matches!(err, BalanceMonitorError::Rpc(_)),
            "expected Rpc error variant, got {err:?}"
        );
    }

    #[test]
    fn test_parse_usdc_malformed_amount() {
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": [{
                    "account": { "data": { "parsed": { "info": { "tokenAmount": {
                        "amount": "not-a-number", "decimals": 6
                    }}}}}
                }]
            }
        });
        let err = parse_usdc_from_json(payload).unwrap_err();
        assert!(
            matches!(err, BalanceMonitorError::Parse(_)),
            "expected Parse error variant, got {err:?}"
        );
    }

    #[test]
    fn test_parse_usdc_saturating_add() {
        // Two ATAs holding u64::MAX each — saturating addition prevents
        // a panic on overflow. This can't happen on real USDC (supply is
        // bounded) but the safety property should hold structurally.
        let max = u64::MAX.to_string();
        let payload = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "value": [
                    { "account": { "data": { "parsed": { "info": { "tokenAmount": {
                        "amount": max, "decimals": 6
                    }}}}}},
                    { "account": { "data": { "parsed": { "info": { "tokenAmount": {
                        "amount": max, "decimals": 6
                    }}}}}}
                ]
            }
        });
        assert_eq!(parse_usdc_from_json(payload).unwrap(), u64::MAX);
    }

    // -------------------------------------------------------------------------
    // AlertSink transition dedup
    // -------------------------------------------------------------------------

    struct RecordingSink {
        events: StdMutex<Vec<BalanceAlertEvent>>,
    }

    impl RecordingSink {
        fn new() -> Self {
            Self {
                events: StdMutex::new(Vec::new()),
            }
        }
        fn snapshot(&self) -> Vec<BalanceAlertEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl AlertSink for RecordingSink {
        async fn dispatch(&self, event: &BalanceAlertEvent) {
            self.events.lock().unwrap().push(event.clone());
        }
    }

    fn make_monitor_with_sink(sink: Arc<dyn AlertSink>) -> BalanceMonitor {
        BalanceMonitor::with_sink(MonitorConfig::default(), vec![], sink)
            .expect("test client builds")
    }

    fn result(pubkey: &str, asset: Asset, level: AlertLevel, balance: f64) -> BalanceCheckResult {
        BalanceCheckResult {
            wallet_pubkey: pubkey.into(),
            asset,
            balance_human: balance,
            atomic_units: 0,
            alert_level: level,
        }
    }

    #[tokio::test]
    async fn test_transition_first_observation_fires() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let results = vec![result("w1", Asset::Sol, AlertLevel::Critical, 0.001)];
        let fired = monitor.dispatch_transitions(&results).await;
        assert_eq!(fired.len(), 1, "first observation should always fire");
        assert_eq!(fired[0].previous_level, None);
        assert_eq!(fired[0].current_level, AlertLevel::Critical);
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn test_transition_same_level_dedups() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let results = vec![result("w1", Asset::Sol, AlertLevel::Critical, 0.001)];
        monitor.dispatch_transitions(&results).await;
        let fired = monitor.dispatch_transitions(&results).await;
        assert!(fired.is_empty(), "second identical level should not fire");
        assert_eq!(
            sink.snapshot().len(),
            1,
            "sink should have been called exactly once"
        );
    }

    #[tokio::test]
    async fn test_transition_level_change_fires() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        // First observation: Critical.
        let crit = vec![result("w1", Asset::Sol, AlertLevel::Critical, 0.001)];
        monitor.dispatch_transitions(&crit).await;
        // Wallet recovers.
        let healthy = vec![result("w1", Asset::Sol, AlertLevel::Healthy, 1.0)];
        let fired = monitor.dispatch_transitions(&healthy).await;
        assert_eq!(fired.len(), 1);
        assert_eq!(fired[0].previous_level, Some(AlertLevel::Critical));
        assert_eq!(fired[0].current_level, AlertLevel::Healthy);
        assert_eq!(sink.snapshot().len(), 2);
    }

    #[tokio::test]
    async fn test_transition_critical_healthy_critical() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        for level in [
            AlertLevel::Critical,
            AlertLevel::Healthy,
            AlertLevel::Critical,
        ] {
            monitor
                .dispatch_transitions(&[result("w1", Asset::Sol, level, 0.0)])
                .await;
        }
        assert_eq!(
            sink.snapshot().len(),
            3,
            "each transition out and back should fire"
        );
    }

    #[tokio::test]
    async fn test_transition_per_asset_dedup_is_independent() {
        // Same wallet, two assets — Critical SOL doesn't dedup Critical USDC.
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let results = vec![
            result("w1", Asset::Sol, AlertLevel::Critical, 0.0),
            result("w1", Asset::Usdc, AlertLevel::Critical, 0.0),
        ];
        let fired = monitor.dispatch_transitions(&results).await;
        assert_eq!(fired.len(), 2);
        let fired_again = monitor.dispatch_transitions(&results).await;
        assert!(
            fired_again.is_empty(),
            "second identical pass should not fire"
        );
        assert_eq!(sink.snapshot().len(), 2);
    }

    #[tokio::test]
    async fn test_noop_sink_does_not_panic() {
        // Constructing the monitor through `new` (which installs NoopSink)
        // must work for the OSS path; transitions should still update the
        // dedup map even though no external dispatch happens.
        let monitor = BalanceMonitor::new(MonitorConfig::default(), vec![]).unwrap();
        let results = vec![result("w1", Asset::Sol, AlertLevel::Critical, 0.0)];
        let fired = monitor.dispatch_transitions(&results).await;
        assert_eq!(fired.len(), 1);
        let fired_again = monitor.dispatch_transitions(&results).await;
        assert!(fired_again.is_empty());
    }

    // -------------------------------------------------------------------------
    // T4 / silent-failure-hunter coverage: empty results + dispatch is a no-op,
    // and dedup state must not be mutated by callers passing in an empty Vec
    // (e.g. an entire RPC outage produces no results).
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_dispatch_transitions_empty_results_is_quiet() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let fired = monitor.dispatch_transitions(&[]).await;
        assert!(fired.is_empty(), "empty input → no events");
        assert!(sink.snapshot().is_empty(), "sink must not be called");
    }

    #[tokio::test]
    async fn test_dispatch_transitions_empty_does_not_disturb_existing_state() {
        // First call: wallet at Critical. Second call: empty Vec — must not
        // wipe the dedup state. Third call (same Critical): still must dedup.
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let crit = vec![result("w1", Asset::Sol, AlertLevel::Critical, 0.0)];
        monitor.dispatch_transitions(&crit).await;
        let empty = monitor.dispatch_transitions(&[]).await;
        assert!(empty.is_empty());
        let again = monitor.dispatch_transitions(&crit).await;
        assert!(
            again.is_empty(),
            "Critical→empty→Critical must still dedup the second Critical"
        );
        assert_eq!(sink.snapshot().len(), 1);
    }

    // -------------------------------------------------------------------------
    // T5 / mixed-asset state — same wallet, SOL Critical, USDC Healthy.
    // The (pubkey, asset) keying must not cross-contaminate.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_transition_mixed_asset_state_no_cross_contamination() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_with_sink(sink.clone());
        let mixed = vec![
            result("w1", Asset::Sol, AlertLevel::Critical, 0.0),
            result("w1", Asset::Usdc, AlertLevel::Healthy, 100.0),
        ];
        let fired = monitor.dispatch_transitions(&mixed).await;
        assert_eq!(fired.len(), 2, "both asset transitions fire");
        // Verify each event carries its own asset, not the other.
        let sol_event = fired.iter().find(|e| e.asset == Asset::Sol).unwrap();
        let usdc_event = fired.iter().find(|e| e.asset == Asset::Usdc).unwrap();
        assert_eq!(sol_event.current_level, AlertLevel::Critical);
        assert_eq!(usdc_event.current_level, AlertLevel::Healthy);
        // Repeat with the same mixed state — both should dedup.
        let again = monitor.dispatch_transitions(&mixed).await;
        assert!(again.is_empty());
        // Now flip just SOL to Healthy; USDC unchanged. Only SOL should fire.
        let flipped = vec![
            result("w1", Asset::Sol, AlertLevel::Healthy, 1.0),
            result("w1", Asset::Usdc, AlertLevel::Healthy, 100.0),
        ];
        let after = monitor.dispatch_transitions(&flipped).await;
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].asset, Asset::Sol);
        assert_eq!(after[0].previous_level, Some(AlertLevel::Critical));
        assert_eq!(after[0].current_level, AlertLevel::Healthy);
    }

    // -------------------------------------------------------------------------
    // C2 — stale-data detection. After N consecutive RPC failures the
    // monitor must emit a synthetic Critical with is_stale = true so the
    // sink can't go silent during an RPC outage.
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn test_stale_fires_after_threshold_failures() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_stale(2, sink.clone());
        // Two failures, threshold = 2.
        monitor.record_failure("w1", Asset::Sol).await;
        monitor.record_failure("w1", Asset::Sol).await;
        let fired = monitor.dispatch_transitions(&[]).await;
        assert_eq!(fired.len(), 1);
        assert!(fired[0].is_stale);
        assert_eq!(fired[0].current_level, AlertLevel::Critical);
        assert_eq!(fired[0].wallet_pubkey, "w1");
        assert_eq!(fired[0].asset, Asset::Sol);
    }

    #[tokio::test]
    async fn test_stale_does_not_double_fire_same_streak() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_stale(1, sink.clone());
        monitor.record_failure("w1", Asset::Sol).await;
        let first = monitor.dispatch_transitions(&[]).await;
        assert_eq!(first.len(), 1);
        assert!(first[0].is_stale);
        // Failure streak continues.
        monitor.record_failure("w1", Asset::Sol).await;
        let second = monitor.dispatch_transitions(&[]).await;
        assert!(
            second.is_empty(),
            "stale must fire once per streak, not every check"
        );
        assert_eq!(sink.snapshot().len(), 1);
    }

    #[tokio::test]
    async fn test_stale_resets_on_recovery_and_can_re_fire() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_stale(1, sink.clone());
        // Outage → stale fires.
        monitor.record_failure("w1", Asset::Sol).await;
        monitor.dispatch_transitions(&[]).await;
        // Recovery — clears the stale flag.
        monitor
            .record_success("w1", Asset::Sol, 1_000_000_000, 1.0)
            .await;
        let recovery = vec![result("w1", Asset::Sol, AlertLevel::Healthy, 1.0)];
        let recovery_fired = monitor.dispatch_transitions(&recovery).await;
        // The wallet was already Critical (set when stale fired); recovering
        // to Healthy is a transition that should fire a non-stale event.
        assert_eq!(recovery_fired.len(), 1);
        assert!(!recovery_fired[0].is_stale);
        assert_eq!(recovery_fired[0].current_level, AlertLevel::Healthy);
        // Outage again — stale must fire again on the new streak.
        monitor.record_failure("w1", Asset::Sol).await;
        let second_outage = monitor.dispatch_transitions(&[]).await;
        assert_eq!(second_outage.len(), 1);
        assert!(second_outage[0].is_stale);
    }

    #[tokio::test]
    async fn test_stale_carries_last_known_good_balance() {
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_stale(1, sink.clone());
        // Record a known-good observation first.
        monitor
            .record_success("w1", Asset::Sol, 500_000_000, 0.5)
            .await;
        let healthy = vec![result("w1", Asset::Sol, AlertLevel::Healthy, 0.5)];
        monitor.dispatch_transitions(&healthy).await;
        // Then an outage.
        monitor.record_failure("w1", Asset::Sol).await;
        let stale = monitor.dispatch_transitions(&[]).await;
        assert_eq!(stale.len(), 1);
        assert!(stale[0].is_stale);
        // The synthetic event should carry the LKG balance so operators
        // know what value preceded the outage.
        assert!((stale[0].balance_human - 0.5).abs() < f64::EPSILON);
        assert_eq!(stale[0].atomic_units, 500_000_000);
        // previous_level must be the Healthy from the dispatch above.
        assert_eq!(stale[0].previous_level, Some(AlertLevel::Healthy));
    }

    #[tokio::test]
    async fn test_stale_zero_when_no_prior_success() {
        // Wallet has never had a successful fetch — stale event reports 0/0.
        let sink = Arc::new(RecordingSink::new());
        let monitor = make_monitor_stale(1, sink.clone());
        monitor.record_failure("w1", Asset::Sol).await;
        let fired = monitor.dispatch_transitions(&[]).await;
        assert_eq!(fired.len(), 1);
        assert!(fired[0].is_stale);
        assert_eq!(fired[0].atomic_units, 0);
        assert!(fired[0].balance_human.abs() < f64::EPSILON);
        assert_eq!(fired[0].previous_level, None);
    }
}
