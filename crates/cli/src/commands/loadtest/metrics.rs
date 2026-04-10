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
                Histogram::new_with_bounds(1, 60_000, 3).expect("histogram bounds are valid"),
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
            RequestOutcome::PaymentRequired402 => {
                self.payment_required_402.fetch_add(1, Ordering::Relaxed)
            }
            RequestOutcome::RateLimited429 => self.rate_limited_429.fetch_add(1, Ordering::Relaxed),
            RequestOutcome::ServerError5xx => {
                self.server_errors_5xx.fetch_add(1, Ordering::Relaxed)
            }
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
            min_ms: if !hist.is_empty() { hist.min() } else { 0 },
            max_ms: if !hist.is_empty() { hist.max() } else { 0 },
            mean_ms: if !hist.is_empty() {
                hist.mean() as u64
            } else {
                0
            },
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
        m.record_outcome(
            RequestOutcome::PaymentRequired402,
            Duration::from_millis(10),
        );
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
        assert!(
            snap.p50_ms >= 45 && snap.p50_ms <= 55,
            "p50 was {}ms",
            snap.p50_ms
        );
        assert!(
            snap.p95_ms >= 90 && snap.p95_ms <= 100,
            "p95 was {}ms",
            snap.p95_ms
        );
        assert!(
            snap.p99_ms >= 95 && snap.p99_ms <= 100,
            "p99 was {}ms",
            snap.p99_ms
        );
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
        assert!(
            (rate - 0.10).abs() < 0.01,
            "error rate should be ~10%, got {rate}"
        );
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
