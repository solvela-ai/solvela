//! Background health checker for external x402 services.
//!
//! Periodically sends HEAD requests to each external service endpoint and
//! updates the `healthy` field on the corresponding `ServiceEntry`. Uses the
//! shared `http_client` from `AppState` with a 10-second per-probe timeout.

use std::sync::Arc;
use std::time::Duration;

use metrics::gauge;
use tokio::sync::watch;
use tracing::{info, warn};

use crate::AppState;

/// Default interval between health check sweeps (seconds).
const DEFAULT_HEALTH_INTERVAL_SECS: u64 = 60;

/// Timeout for each individual HEAD probe.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Start the background service health checker.
///
/// Spawns a `tokio::spawn` loop that runs every `interval` seconds (default 60,
/// configurable via `SOLVELA_SERVICE_HEALTH_INTERVAL_SECS`). For each external,
/// x402-enabled service, sends a HEAD request and marks it healthy or unhealthy
/// in the `ServiceRegistry`.
///
/// The task shuts down gracefully when `shutdown_rx` receives `true`.
pub fn start_service_health_checker(
    state: Arc<AppState>,
    mut shutdown_rx: watch::Receiver<bool>,
) -> tokio::task::JoinHandle<()> {
    let interval_secs: u64 = std::env::var("SOLVELA_SERVICE_HEALTH_INTERVAL_SECS")
        .or_else(|_| std::env::var("RCR_SERVICE_HEALTH_INTERVAL_SECS"))
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_HEALTH_INTERVAL_SECS);

    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));

        info!(interval_secs, "service health checker started");

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    check_all_services(&state).await;
                }
                _ = shutdown_rx.changed() => {
                    info!("service health checker shutting down gracefully");
                    break;
                }
            }
        }
    })
}

/// Probe all external services and update their health status.
async fn check_all_services(state: &AppState) {
    // Collect the list of external services (read lock, released immediately).
    let external_services: Vec<(String, String)> = {
        let registry = state.service_registry.read().await;
        registry
            .external()
            .iter()
            .filter(|s| s.x402_enabled)
            .map(|s| (s.id.clone(), s.endpoint.clone()))
            .collect()
    };

    if external_services.is_empty() {
        return;
    }

    for (service_id, endpoint) in &external_services {
        let healthy = probe_service(&state.http_client, endpoint).await;

        if !healthy {
            warn!(
                service_id = %service_id,
                endpoint = %endpoint,
                "service health check failed — marking unhealthy"
            );
        }

        gauge!("solvela_service_health", "service_id" => service_id.to_string()).set(if healthy {
            1.0
        } else {
            0.0
        });

        // Acquire write lock briefly to update health status.
        let mut registry = state.service_registry.write().await;
        registry.set_health(service_id, healthy);
    }
}

/// Send a HEAD request to the service endpoint and determine health.
///
/// Returns `true` if the response is 2xx, 402 (payment required — service is
/// alive), or 405 (method not allowed — service exists but rejects HEAD).
/// Returns `false` on connection error, timeout, or 5xx.
async fn probe_service(client: &reqwest::Client, endpoint: &str) -> bool {
    let result = client.head(endpoint).timeout(PROBE_TIMEOUT).send().await;

    match result {
        Ok(response) => {
            let status = response.status();
            // 2xx, 402, or 405 all indicate the service is reachable.
            status.is_success() || status.as_u16() == 402 || status.as_u16() == 405
        }
        Err(_) => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::services::ServiceRegistry;

    #[test]
    fn test_set_health_updates_entry() {
        let toml = r#"
[services.ext-svc]
name = "External"
endpoint = "https://example.com/api"
category = "data"
x402_enabled = true
internal = false
price_per_request_usdc = 0.01
"#;
        let mut registry = ServiceRegistry::from_toml(toml).unwrap();
        assert_eq!(registry.get("ext-svc").unwrap().healthy, None);

        assert!(registry.set_health("ext-svc", true));
        assert_eq!(registry.get("ext-svc").unwrap().healthy, Some(true));

        assert!(registry.set_health("ext-svc", false));
        assert_eq!(registry.get("ext-svc").unwrap().healthy, Some(false));
    }

    #[test]
    fn test_set_health_unknown_service_returns_false() {
        let mut registry = ServiceRegistry::empty();
        assert!(!registry.set_health("nonexistent", true));
    }
}
