use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};

use solvela_x402::types::PaymentRequired;

use super::metrics::{MetricsCollector, RequestOutcome};
use super::payment::PaymentStrategy;

/// Execute a single load test request: the full 402-dance lifecycle.
///
/// 1. POST to /v1/chat/completions
/// 2. If 200 -> record success
/// 3. If 402 -> parse `PaymentRequired`, call `strategy.prepare_payment`, retry with header
/// 4. If 429/5xx/other -> record error category
///
/// Latency is measured from `scheduled_at` (the dispatcher's tick instant),
/// NOT from when this function starts. This includes any queuing delay and
/// prevents coordinated omission bias in the reported percentiles.
pub async fn execute_request(
    client: &reqwest::Client,
    api_url: &str,
    body: &serde_json::Value,
    strategy: &dyn PaymentStrategy,
    rpc_url: &str,
    metrics: &Arc<MetricsCollector>,
    scheduled_at: Instant,
) -> Result<()> {
    let endpoint = format!("{api_url}/v1/chat/completions");

    // First request (may return 402).
    let resp = match client.post(&endpoint).json(body).send().await {
        Ok(r) => r,
        Err(e) => {
            let latency = scheduled_at.elapsed();
            if e.is_timeout() {
                metrics.record_outcome(RequestOutcome::Timeout, latency);
            } else {
                metrics.record_outcome(RequestOutcome::OtherError, latency);
            }
            return Err(e.into());
        }
    };

    let status = resp.status().as_u16();

    match status {
        200..=299 => {
            // Success without payment (dev-bypass mode or free model).
            let latency = scheduled_at.elapsed();
            metrics.record_success(latency);
        }
        402 => {
            // Payment required — execute the 402 dance.
            let error_body: serde_json::Value = resp
                .json()
                .await
                .context("failed to parse 402 response body")?;

            let error_msg = error_body["error"]["message"].as_str().unwrap_or("");

            let payment_required: PaymentRequired = serde_json::from_str(error_msg)
                .context("failed to parse PaymentRequired from 402")?;

            let payment_header = strategy
                .prepare_payment(rpc_url, body, &payment_required.accepts)
                .await
                .context("payment strategy failed")?;

            match payment_header {
                Some(header_value) => {
                    // Retry with payment.
                    let retry_resp = match client
                        .post(&endpoint)
                        .header("PAYMENT-SIGNATURE", &header_value)
                        .json(body)
                        .send()
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            let latency = scheduled_at.elapsed();
                            if e.is_timeout() {
                                metrics.record_outcome(RequestOutcome::Timeout, latency);
                            } else {
                                metrics.record_outcome(RequestOutcome::OtherError, latency);
                            }
                            return Err(e.into());
                        }
                    };

                    let retry_status = retry_resp.status().as_u16();
                    let latency = scheduled_at.elapsed();

                    match retry_status {
                        200..=299 => metrics.record_success(latency),
                        429 => metrics.record_outcome(RequestOutcome::RateLimited429, latency),
                        500..=599 => {
                            metrics.record_outcome(RequestOutcome::ServerError5xx, latency);
                        }
                        _ => metrics.record_outcome(RequestOutcome::OtherError, latency),
                    }
                }
                None => {
                    // DevBypass mode: 402 means the gateway isn't in bypass mode.
                    // Record as 402 error.
                    let latency = scheduled_at.elapsed();
                    metrics.record_outcome(RequestOutcome::PaymentRequired402, latency);
                }
            }
        }
        429 => {
            let latency = scheduled_at.elapsed();
            metrics.record_outcome(RequestOutcome::RateLimited429, latency);
        }
        500..=599 => {
            let latency = scheduled_at.elapsed();
            metrics.record_outcome(RequestOutcome::ServerError5xx, latency);
        }
        _ => {
            let latency = scheduled_at.elapsed();
            metrics.record_outcome(RequestOutcome::OtherError, latency);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::metrics::MetricsCollector;
    use crate::commands::loadtest::payment::DevBypassStrategy;
    use std::sync::Arc;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_body() -> serde_json::Value {
        serde_json::json!({
            "model": "auto",
            "messages": [{"role": "user", "content": "test prompt"}]
        })
    }

    #[tokio::test]
    async fn test_worker_success_on_200() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "hello"}}]
            })))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let outcome = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        assert!(outcome.is_ok());
        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.successful, 1);
    }

    #[tokio::test]
    async fn test_worker_records_5xx() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500).set_body_string("internal error"))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.server_errors_5xx, 1);
    }

    #[tokio::test]
    async fn test_worker_records_429() {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(429).set_body_string("rate limited"))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.rate_limited_429, 1);
    }

    #[tokio::test]
    async fn test_worker_connection_error() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind to ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        drop(listener);
        let dead_url = format!("http://127.0.0.1:{port}");

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &dead_url,
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.other_errors, 1);
    }

    /// Mock strategy that always returns a stub payment header.
    struct MockPaymentStrategy;

    #[async_trait::async_trait]
    impl PaymentStrategy for MockPaymentStrategy {
        fn name(&self) -> &'static str {
            "mock"
        }

        async fn prepare_payment(
            &self,
            _rpc_url: &str,
            _request_body: &serde_json::Value,
            _accepts: &[solvela_x402::types::PaymentAccept],
        ) -> anyhow::Result<Option<String>> {
            Ok(Some("stub-payment-header".to_string()))
        }
    }

    #[tokio::test]
    async fn test_worker_402_dance_with_payment() {
        use wiremock::matchers::header_exists;

        let mock = MockServer::start().await;

        // Mount the 200 response for requests WITH the payment header first
        // (more specific matcher takes priority in wiremock).
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(header_exists("PAYMENT-SIGNATURE"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [{"message": {"content": "paid response"}}]
            })))
            .expect(1)
            .mount(&mock)
            .await;

        // Fallback: any POST without the header -> 402 with PaymentRequired body.
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": "{\"x402_version\":1,\"resource\":{\"url\":\"/v1/chat/completions\",\"method\":\"POST\"},\"accepts\":[{\"scheme\":\"exact\",\"network\":\"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp\",\"amount\":\"1000\",\"asset\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\",\"pay_to\":\"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM\",\"max_timeout_seconds\":300}],\"cost_breakdown\":{\"provider_cost\":\"0.001000\",\"platform_fee\":\"0.000050\",\"total\":\"0.001050\",\"currency\":\"USDC\",\"fee_percent\":5},\"error\":\"Payment required\"}"
                }
            })))
            .expect(1)
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(MockPaymentStrategy);

        let outcome = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        assert!(
            outcome.is_ok(),
            "402-dance should succeed: {:?}",
            outcome.err()
        );
        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(
            snap.successful, 1,
            "request should be recorded as successful after 402 dance"
        );
    }

    #[tokio::test]
    async fn test_worker_402_dev_bypass_records_payment_required() {
        // DevBypass returns None for payment, so a 402 response should be
        // recorded as PaymentRequired402 (gateway not in bypass mode).
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
                "error": {
                    "message": "{\"x402_version\":1,\"resource\":{\"url\":\"/v1/chat/completions\",\"method\":\"POST\"},\"accepts\":[{\"scheme\":\"exact\",\"network\":\"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp\",\"amount\":\"1000\",\"asset\":\"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v\",\"pay_to\":\"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM\",\"max_timeout_seconds\":300}],\"cost_breakdown\":{\"provider_cost\":\"0.001000\",\"platform_fee\":\"0.000050\",\"total\":\"0.001050\",\"currency\":\"USDC\",\"fee_percent\":5},\"error\":\"Payment required\"}"
                }
            })))
            .mount(&mock)
            .await;

        let client = reqwest::Client::new();
        let metrics = Arc::new(MetricsCollector::new());
        let strategy: Arc<dyn PaymentStrategy> = Arc::new(DevBypassStrategy);

        let _ = execute_request(
            &client,
            &mock.uri(),
            &test_body(),
            strategy.as_ref(),
            "",
            &metrics,
            Instant::now(),
        )
        .await;

        let snap = metrics.snapshot();
        assert_eq!(snap.total_requests, 1);
        assert_eq!(snap.payment_required_402, 1);
    }
}
