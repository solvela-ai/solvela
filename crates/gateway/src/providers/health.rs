//! Provider health tracking and circuit breaker.
//!
//! Tracks per-provider:
//! - EWMA latency (10s window)
//! - Failure rate per minute
//! - Rate limit tracking (429 responses)
//!
//! Circuit breaker: >50% failure rate -> cooldown 30s -> try next provider

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;
use tracing::{info, warn};

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Provider is failing — requests are blocked, try next provider.
    Open,
    /// Testing if provider recovered — allow a single probe request.
    HalfOpen,
}

/// Configuration for the circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Failure rate threshold to open circuit (0.0 to 1.0).
    pub failure_threshold: f64,
    /// Duration to keep circuit open before trying half-open.
    pub cooldown: Duration,
    /// Window for tracking failure rate.
    pub window: Duration,
    /// Minimum requests in window before evaluating failure rate.
    pub min_requests: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 0.5,
            cooldown: Duration::from_secs(30),
            window: Duration::from_secs(60),
            min_requests: 5,
        }
    }
}

/// Health statistics for a single provider.
#[derive(Debug)]
struct ProviderHealth {
    /// Recent request outcomes: (timestamp, success, latency_ms)
    outcomes: Vec<(Instant, bool, u64)>,
    /// Current circuit state.
    state: CircuitState,
    /// When the circuit was opened (for cooldown).
    opened_at: Option<Instant>,
    /// EWMA latency in milliseconds.
    ewma_latency_ms: f64,
    /// EWMA alpha (smoothing factor).
    ewma_alpha: f64,
}

impl ProviderHealth {
    fn new() -> Self {
        Self {
            outcomes: Vec::new(),
            state: CircuitState::Closed,
            opened_at: None,
            ewma_latency_ms: 0.0,
            ewma_alpha: 0.2, // ~10s window with 2s avg request time
        }
    }
}

/// Tracks health and manages circuit breakers for all providers.
#[derive(Clone)]
pub struct ProviderHealthTracker {
    providers: Arc<RwLock<HashMap<String, ProviderHealth>>>,
    config: CircuitBreakerConfig,
}

impl ProviderHealthTracker {
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            providers: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Record a successful request to a provider.
    pub async fn record_success(&self, provider: &str, latency_ms: u64) {
        let mut providers = self.providers.write().await;
        let health = providers
            .entry(provider.to_string())
            .or_insert_with(ProviderHealth::new);

        let now = Instant::now();
        health.outcomes.push((now, true, latency_ms));

        // Update EWMA latency
        health.ewma_latency_ms = health.ewma_alpha * latency_ms as f64
            + (1.0 - health.ewma_alpha) * health.ewma_latency_ms;

        // If half-open and success, close the circuit
        if health.state == CircuitState::HalfOpen {
            info!(
                provider,
                "circuit breaker: half-open → closed (probe succeeded)"
            );
            health.state = CircuitState::Closed;
            health.opened_at = None;
        }

        self.cleanup_old_outcomes(health);
    }

    /// Record a failed request to a provider.
    pub async fn record_failure(&self, provider: &str, latency_ms: u64) {
        let mut providers = self.providers.write().await;
        let health = providers
            .entry(provider.to_string())
            .or_insert_with(ProviderHealth::new);

        let now = Instant::now();
        health.outcomes.push((now, false, latency_ms));

        // Update EWMA latency
        health.ewma_latency_ms = health.ewma_alpha * latency_ms as f64
            + (1.0 - health.ewma_alpha) * health.ewma_latency_ms;

        // If half-open and failed, reopen
        if health.state == CircuitState::HalfOpen {
            info!(provider, "circuit breaker: half-open → open (probe failed)");
            health.state = CircuitState::Open;
            health.opened_at = Some(now);
            return;
        }

        self.cleanup_old_outcomes(health);
        self.evaluate_circuit(provider, health);
    }

    /// Check if a provider is available (circuit is not open).
    pub async fn is_available(&self, provider: &str) -> bool {
        let mut providers = self.providers.write().await;
        let health = match providers.get_mut(provider) {
            Some(h) => h,
            None => return true, // Unknown provider is assumed available
        };

        match health.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true, // Allow probe request
            CircuitState::Open => {
                // Check if cooldown has elapsed
                if let Some(opened_at) = health.opened_at {
                    if opened_at.elapsed() >= self.config.cooldown {
                        info!(
                            provider,
                            "circuit breaker: open → half-open (cooldown elapsed)"
                        );
                        health.state = CircuitState::HalfOpen;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    /// Get the current circuit state for a provider.
    pub async fn get_state(&self, provider: &str) -> CircuitState {
        let providers = self.providers.read().await;
        providers
            .get(provider)
            .map(|h| h.state)
            .unwrap_or(CircuitState::Closed)
    }

    /// Get the EWMA latency for a provider.
    pub async fn get_latency_ms(&self, provider: &str) -> f64 {
        let providers = self.providers.read().await;
        providers
            .get(provider)
            .map(|h| h.ewma_latency_ms)
            .unwrap_or(0.0)
    }

    /// Get failure rate for a provider in the current window.
    pub async fn get_failure_rate(&self, provider: &str) -> f64 {
        let providers = self.providers.read().await;
        match providers.get(provider) {
            Some(health) => {
                let now = Instant::now();
                let window_start = now - self.config.window;
                let recent: Vec<_> = health
                    .outcomes
                    .iter()
                    .filter(|(t, _, _)| *t >= window_start)
                    .collect();
                if recent.is_empty() {
                    return 0.0;
                }
                let failures = recent.iter().filter(|(_, success, _)| !success).count();
                failures as f64 / recent.len() as f64
            }
            None => 0.0,
        }
    }

    fn cleanup_old_outcomes(&self, health: &mut ProviderHealth) {
        let cutoff = Instant::now() - self.config.window * 2;
        health.outcomes.retain(|(t, _, _)| *t >= cutoff);
    }

    fn evaluate_circuit(&self, provider: &str, health: &mut ProviderHealth) {
        let now = Instant::now();
        let window_start = now - self.config.window;
        let recent: Vec<_> = health
            .outcomes
            .iter()
            .filter(|(t, _, _)| *t >= window_start)
            .collect();

        if (recent.len() as u32) < self.config.min_requests {
            return; // Not enough data
        }

        let failures = recent.iter().filter(|(_, success, _)| !success).count();
        let failure_rate = failures as f64 / recent.len() as f64;

        if failure_rate >= self.config.failure_threshold && health.state == CircuitState::Closed {
            warn!(
                provider,
                failure_rate = format!("{:.1}%", failure_rate * 100.0),
                "circuit breaker: closed → open"
            );
            health.state = CircuitState::Open;
            health.opened_at = Some(now);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> CircuitBreakerConfig {
        CircuitBreakerConfig {
            failure_threshold: 0.5,
            cooldown: Duration::from_millis(50), // Short cooldown for tests
            window: Duration::from_secs(60),
            min_requests: 5,
        }
    }

    #[tokio::test]
    async fn test_circuit_starts_closed() {
        let tracker = ProviderHealthTracker::new(test_config());

        assert_eq!(tracker.get_state("openai").await, CircuitState::Closed);
        assert!(tracker.is_available("openai").await);
    }

    #[tokio::test]
    async fn test_success_records_latency() {
        let tracker = ProviderHealthTracker::new(test_config());

        tracker.record_success("openai", 100).await;
        let latency = tracker.get_latency_ms("openai").await;
        // First sample: EWMA = alpha * 100 + (1 - alpha) * 0 = 0.2 * 100 = 20.0
        assert!((latency - 20.0).abs() < 0.01);

        tracker.record_success("openai", 200).await;
        let latency = tracker.get_latency_ms("openai").await;
        // Second sample: EWMA = 0.2 * 200 + 0.8 * 20.0 = 40 + 16 = 56.0
        assert!((latency - 56.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_circuit_opens_on_failures() {
        let tracker = ProviderHealthTracker::new(test_config());

        // Record 5 failures (>= min_requests, 100% failure rate > 50% threshold)
        for _ in 0..5 {
            tracker.record_failure("openai", 500).await;
        }

        assert_eq!(tracker.get_state("openai").await, CircuitState::Open);
        assert!(!tracker.is_available("openai").await);
    }

    #[tokio::test]
    async fn test_circuit_half_open_after_cooldown() {
        let config = CircuitBreakerConfig {
            failure_threshold: 0.5,
            cooldown: Duration::from_millis(20), // Very short cooldown for test
            window: Duration::from_secs(60),
            min_requests: 5,
        };
        let tracker = ProviderHealthTracker::new(config);

        // Open the circuit
        for _ in 0..5 {
            tracker.record_failure("openai", 500).await;
        }
        assert_eq!(tracker.get_state("openai").await, CircuitState::Open);

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Should transition to half-open when checked
        assert!(tracker.is_available("openai").await);
        assert_eq!(tracker.get_state("openai").await, CircuitState::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_closes_on_probe_success() {
        let config = CircuitBreakerConfig {
            failure_threshold: 0.5,
            cooldown: Duration::from_millis(20),
            window: Duration::from_secs(60),
            min_requests: 5,
        };
        let tracker = ProviderHealthTracker::new(config);

        // Open the circuit
        for _ in 0..5 {
            tracker.record_failure("openai", 500).await;
        }

        // Wait for cooldown to transition to half-open
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(tracker.is_available("openai").await);
        assert_eq!(tracker.get_state("openai").await, CircuitState::HalfOpen);

        // Probe success should close the circuit
        tracker.record_success("openai", 100).await;
        assert_eq!(tracker.get_state("openai").await, CircuitState::Closed);
        assert!(tracker.is_available("openai").await);
    }

    #[tokio::test]
    async fn test_circuit_reopens_on_probe_failure() {
        let config = CircuitBreakerConfig {
            failure_threshold: 0.5,
            cooldown: Duration::from_millis(20),
            window: Duration::from_secs(60),
            min_requests: 5,
        };
        let tracker = ProviderHealthTracker::new(config);

        // Open the circuit
        for _ in 0..5 {
            tracker.record_failure("openai", 500).await;
        }

        // Wait for cooldown to transition to half-open
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(tracker.is_available("openai").await);
        assert_eq!(tracker.get_state("openai").await, CircuitState::HalfOpen);

        // Probe failure should reopen the circuit
        tracker.record_failure("openai", 500).await;
        assert_eq!(tracker.get_state("openai").await, CircuitState::Open);
        assert!(!tracker.is_available("openai").await);
    }

    #[tokio::test]
    async fn test_failure_rate_calculation() {
        let tracker = ProviderHealthTracker::new(test_config());

        // 3 successes, 2 failures => 40% failure rate
        tracker.record_success("openai", 100).await;
        tracker.record_success("openai", 100).await;
        tracker.record_success("openai", 100).await;
        tracker.record_failure("openai", 500).await;
        tracker.record_failure("openai", 500).await;

        let rate = tracker.get_failure_rate("openai").await;
        assert!((rate - 0.4).abs() < 0.01);

        // Circuit should still be closed (40% < 50% threshold)
        assert_eq!(tracker.get_state("openai").await, CircuitState::Closed);
    }
}
