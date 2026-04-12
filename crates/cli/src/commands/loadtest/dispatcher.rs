use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Semaphore;
use tokio::time;

use super::config::TierWeights;
use super::metrics::MetricsCollector;

/// Configuration for the constant-arrival-rate dispatcher.
pub struct DispatcherConfig {
    pub rps: u64,
    pub duration_secs: u64,
    pub concurrency: usize,
}

/// Select a tier based on weighted random distribution.
///
/// Uses `getrandom` for a uniform random byte, then maps to a tier
/// via cumulative weight ranges.
fn select_tier(weights: &TierWeights) -> &'static str {
    let r = rand_u8() % 100; // 0-99 inclusive
    if r < weights.simple {
        "simple"
    } else if r < weights.simple + weights.medium {
        "medium"
    } else if r < weights.simple + weights.medium + weights.complex {
        "complex"
    } else {
        "reasoning"
    }
}

/// Generate a single random byte via `getrandom`.
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
/// dropped (counted via [`MetricsCollector::record_dropped`]) rather than
/// blocking the dispatch loop -- this prevents coordinated omission bias.
///
/// Latency is measured from the `interval.tick()` instant (`scheduled_at`),
/// NOT from when the worker starts, so queuing delay at the semaphore is
/// included in reported percentiles.
///
/// The `worker_fn` factory is called for each dispatched request. It receives
/// the scheduled-at instant, the selected tier name, and the shared metrics
/// collector. The returned future runs inside a spawned task.
pub async fn run_dispatcher<F, Fut>(
    config: DispatcherConfig,
    tier_weights: TierWeights,
    metrics: Arc<MetricsCollector>,
    worker_fn: F,
) where
    F: Fn(Instant, &'static str, Arc<MetricsCollector>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let total_requests = config.rps * config.duration_secs;
    let interval_duration = Duration::from_secs_f64(1.0 / config.rps as f64);
    let semaphore = Arc::new(Semaphore::new(config.concurrency));

    let mut interval = time::interval(interval_duration);
    // Skip missed ticks instead of bursting -- prevents thundering herd
    // after semaphore saturation.
    interval.set_missed_tick_behavior(time::MissedTickBehavior::Skip);

    let mut join_set = tokio::task::JoinSet::new();
    let worker_fn = Arc::new(worker_fn);

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

    for _i in 0..total_requests {
        interval.tick().await;

        // Record scheduled time BEFORE attempting semaphore acquire.
        // This is the latency start -- includes any queuing delay.
        let scheduled_at = Instant::now();

        // Non-blocking acquire: if all permits are taken, drop the request.
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(p) => p,
            Err(_) => {
                metrics.record_dropped();
                continue;
            }
        };

        let tier = select_tier(&tier_weights);
        let metrics = metrics.clone();
        let worker_fn = worker_fn.clone();

        join_set.spawn(async move {
            worker_fn(scheduled_at, tier, metrics).await;
            drop(permit); // Release semaphore slot.
        });
    }

    // Wait for all in-flight requests to complete.
    while join_set.join_next().await.is_some() {}

    progress_handle.abort();
    eprintln!(); // newline after progress line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Placeholder worker that completes instantly and records success.
    async fn fast_worker(
        scheduled_at: Instant,
        _tier: &'static str,
        metrics: Arc<MetricsCollector>,
    ) {
        let latency = scheduled_at.elapsed();
        metrics.record_success(latency);
    }

    /// Placeholder worker that sleeps 200ms to simulate slow responses.
    async fn slow_worker(
        scheduled_at: Instant,
        _tier: &'static str,
        metrics: Arc<MetricsCollector>,
    ) {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let latency = scheduled_at.elapsed();
        metrics.record_success(latency);
    }

    #[tokio::test]
    async fn test_dispatcher_sends_correct_count() {
        let metrics = Arc::new(MetricsCollector::new());

        let config = DispatcherConfig {
            rps: 10,
            duration_secs: 1,
            concurrency: 20,
        };

        run_dispatcher(config, TierWeights::default(), metrics.clone(), fast_worker).await;

        let snap = metrics.snapshot();
        // Should dispatch exactly rps * duration = 10 requests.
        // With 20 concurrency slots and fast responses, none should be dropped.
        assert_eq!(
            snap.total_requests, 10,
            "expected 10 requests, got {}",
            snap.total_requests
        );
        assert_eq!(snap.successful, 10);
        assert_eq!(
            snap.dropped_requests, 0,
            "no requests should be dropped with excess concurrency"
        );
    }

    #[tokio::test]
    async fn test_dispatcher_respects_concurrency_limit() {
        let metrics = Arc::new(MetricsCollector::new());

        // 50 RPS but only 5 concurrent slots with 200ms latency.
        // With try_acquire_owned, requests beyond the 5 in-flight slots
        // are dropped rather than queued.
        let config = DispatcherConfig {
            rps: 50,
            duration_secs: 1,
            concurrency: 5,
        };

        run_dispatcher(config, TierWeights::default(), metrics.clone(), slow_worker).await;

        let snap = metrics.snapshot();
        // Some requests should complete successfully.
        assert!(
            snap.total_requests > 0,
            "should have dispatched some requests"
        );
        // With 200ms latency and only 5 slots, many of the 50 requests
        // should be dropped because try_acquire_owned fails.
        assert!(
            snap.dropped_requests > 0,
            "expected some dropped requests under semaphore saturation, got 0"
        );
        // Total completed + dropped should account for all 50 attempts.
        assert_eq!(
            snap.total_requests + snap.dropped_requests,
            50,
            "completed ({}) + dropped ({}) should equal total attempts (50)",
            snap.total_requests,
            snap.dropped_requests
        );
    }

    #[tokio::test]
    async fn test_dispatcher_selects_tiers() {
        // Verify tier selection produces valid tier names.
        let weights = TierWeights::default();
        let valid = ["simple", "medium", "complex", "reasoning"];
        for _ in 0..100 {
            let tier = select_tier(&weights);
            assert!(valid.contains(&tier), "unexpected tier: {tier}");
        }
    }

    #[tokio::test]
    async fn test_dispatcher_passes_scheduled_at_before_work() {
        // Verify that scheduled_at is captured before worker runs,
        // so latency includes queuing time.
        let metrics = Arc::new(MetricsCollector::new());
        let call_count = Arc::new(AtomicU64::new(0));

        let counter = call_count.clone();
        let worker =
            move |scheduled_at: Instant, _tier: &'static str, metrics: Arc<MetricsCollector>| {
                let counter = counter.clone();
                async move {
                    // Simulate a small delay.
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    let latency = scheduled_at.elapsed();
                    // Latency should be >= 5ms since we slept.
                    assert!(
                        latency >= Duration::from_millis(4),
                        "latency should include sleep time, got {:?}",
                        latency
                    );
                    metrics.record_success(latency);
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            };

        let config = DispatcherConfig {
            rps: 5,
            duration_secs: 1,
            concurrency: 10,
        };

        run_dispatcher(config, TierWeights::default(), metrics.clone(), worker).await;

        assert_eq!(
            call_count.load(Ordering::Relaxed),
            5,
            "worker should have been called 5 times"
        );
    }
}
