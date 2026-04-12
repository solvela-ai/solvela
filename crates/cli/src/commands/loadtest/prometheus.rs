//! Prometheus metrics scraper and SLO validator for load tests.
//!
//! Scrapes the gateway's `/metrics` endpoint before and after a load test to
//! compute per-metric deltas, enabling cross-validation against the load test's
//! internal `MetricsCollector` counts.
//!
//! Parsing is intentionally simple: only counter and gauge lines are extracted.
//! Histogram buckets (`_bucket`, `_sum`, `_count` suffixes) are parsed as-is
//! since we only do delta math — no semantic interpretation is needed.

use std::collections::HashMap;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::commands::loadtest::config::SloThresholds;
use crate::commands::loadtest::metrics::MetricsSnapshot;

/// Result of a single SLO check.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct SloCheckResult {
    /// Human-readable label for the check (e.g. "p99 latency").
    pub label: String,
    /// Whether the check passed.
    pub passed: bool,
    /// Measured value.
    pub measured: f64,
    /// Threshold that was compared against.
    pub threshold: f64,
    /// Unit string for display (e.g. "ms", "%").
    pub unit: &'static str,
}

#[allow(dead_code)]
impl SloCheckResult {
    /// Short display string: "PASS" or "FAIL".
    pub fn status(&self) -> &'static str {
        if self.passed {
            "PASS"
        } else {
            "FAIL"
        }
    }
}

/// Parse a single Prometheus exposition format line into (metric_name, value).
///
/// Returns `None` for comments, `# TYPE` / `# HELP` lines, empty lines, and
/// lines with non-finite or unparseable values. The metric name is the bare
/// name without labels — labels in `{...}` are stripped.
///
/// Handles both labelled (`name{k="v"} val`) and unlabelled (`name val`) forms.
pub fn parse_metric_line(line: &str) -> Option<(String, f64)> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    // Locate where the metric name ends: either at '{' (labels) or ' ' (no labels).
    let name_end = line.find('{').or_else(|| line.find(' '))?;
    let name = line[..name_end].to_string();

    // Find where the value starts: after '}' + space, or after name + space.
    let value_start = if let Some(brace_end) = line.find('}') {
        brace_end + 2 // skip '} '
    } else {
        name_end + 1
    };

    if value_start >= line.len() {
        return None;
    }

    // Value is the first whitespace-delimited token; ignore optional timestamp.
    let value_str = line[value_start..].split_whitespace().next()?;

    // Skip special Prometheus float tokens that would parse to non-finite f64.
    if matches!(value_str, "+Inf" | "-Inf" | "Inf" | "NaN") {
        return None;
    }

    let value: f64 = value_str.parse().ok()?;
    Some((name, value))
}

/// Scrape a Prometheus `/metrics` endpoint.
///
/// Makes an HTTP GET with a 5-second timeout. On network error or non-200
/// status the error is returned (callers should warn and continue rather than
/// abort the load test).
pub async fn scrape_metrics(url: &str) -> Result<Vec<(String, f64)>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .context("failed to build HTTP client for Prometheus scrape")?;

    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to connect to Prometheus endpoint at {url}"))?;

    if !resp.status().is_success() {
        anyhow::bail!("Prometheus scrape returned HTTP {}: {url}", resp.status());
    }

    let text = resp
        .text()
        .await
        .context("failed to read Prometheus response body")?;

    let metrics: Vec<(String, f64)> = text.lines().filter_map(parse_metric_line).collect();

    Ok(metrics)
}

/// Compute per-metric deltas between two scrapes.
///
/// For each metric present in `after`, subtracts the corresponding `before`
/// value (defaulting to 0.0 for metrics that appeared only in `after`).
/// Metrics present only in `before` are omitted (they did not change or
/// were reset, both of which are unusual for counters).
pub fn compute_deltas(before: &[(String, f64)], after: &[(String, f64)]) -> HashMap<String, f64> {
    let before_map: HashMap<&str, f64> = before.iter().map(|(k, v)| (k.as_str(), *v)).collect();

    after
        .iter()
        .map(|(name, after_val)| {
            let before_val = before_map.get(name.as_str()).copied().unwrap_or(0.0);
            (name.clone(), after_val - before_val)
        })
        .collect()
}

/// Validate a `MetricsSnapshot` against `SloThresholds`.
///
/// Returns one `SloCheckResult` per SLO dimension checked.
#[allow(dead_code)]
pub fn validate_slos(
    snapshot: &MetricsSnapshot,
    thresholds: &SloThresholds,
) -> Vec<SloCheckResult> {
    let p99_measured = snapshot.p99_ms as f64;
    let p99_threshold = thresholds.p99_ms as f64;
    let error_rate_measured = snapshot.error_rate() * 100.0;
    let error_rate_threshold = thresholds.error_rate * 100.0;

    vec![
        SloCheckResult {
            label: "p99 latency".to_string(),
            passed: snapshot.p99_ms <= thresholds.p99_ms,
            measured: p99_measured,
            threshold: p99_threshold,
            unit: "ms",
        },
        SloCheckResult {
            label: "error rate".to_string(),
            passed: snapshot.error_rate() <= thresholds.error_rate,
            measured: error_rate_measured,
            threshold: error_rate_threshold,
            unit: "%",
        },
    ]
}

/// Print a summary of Prometheus metric deltas to stdout.
///
/// Only metrics whose delta is non-zero are printed to reduce noise.
pub fn print_prometheus_deltas(deltas: &HashMap<String, f64>) {
    if deltas.is_empty() {
        println!("  (no Prometheus metrics found)");
        return;
    }

    println!();
    println!("  --- Prometheus Deltas ---");

    let mut sorted: Vec<(&String, &f64)> = deltas.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());

    for (name, delta) in sorted {
        if *delta != 0.0 {
            println!("  {name}: {delta:+.0}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::config::SloThresholds;
    use crate::commands::loadtest::metrics::MetricsCollector;

    // --- parse_metric_line ---

    #[test]
    fn test_parse_prometheus_line_counter() {
        let line = r#"solvela_http_requests_total{method="POST",path="/v1/chat/completions"} 42"#;
        let parsed = parse_metric_line(line);
        assert_eq!(
            parsed,
            Some(("solvela_http_requests_total".to_string(), 42.0))
        );
    }

    #[test]
    fn test_parse_prometheus_line_skips_comments() {
        let line = "# HELP solvela_http_requests_total Total requests";
        let parsed = parse_metric_line(line);
        assert!(parsed.is_none());
    }

    #[test]
    fn test_parse_prometheus_line_skips_type_annotation() {
        let line = "# TYPE solvela_http_requests_total counter";
        assert!(parse_metric_line(line).is_none());
    }

    #[test]
    fn test_parse_prometheus_line_unlabelled_gauge() {
        let line = "solvela_active_requests 7";
        let parsed = parse_metric_line(line);
        assert_eq!(parsed, Some(("solvela_active_requests".to_string(), 7.0)));
    }

    #[test]
    fn test_parse_prometheus_line_float_value() {
        let line = "solvela_request_duration_seconds_sum 0.123456";
        let parsed = parse_metric_line(line);
        assert_eq!(
            parsed,
            Some((
                "solvela_request_duration_seconds_sum".to_string(),
                0.123_456
            ))
        );
    }

    #[test]
    fn test_parse_prometheus_line_skips_inf() {
        let line = r#"solvela_request_duration_seconds_bucket{le="+Inf"} 1000"#;
        // The metric name parses fine; the value token after '}' is "1000", not "+Inf".
        // "+Inf" is inside the label, so this should parse successfully.
        let parsed = parse_metric_line(line);
        assert_eq!(
            parsed,
            Some((
                "solvela_request_duration_seconds_bucket".to_string(),
                1000.0
            ))
        );
    }

    #[test]
    fn test_parse_prometheus_line_skips_empty() {
        assert!(parse_metric_line("").is_none());
        assert!(parse_metric_line("   ").is_none());
    }

    // --- compute_deltas ---

    #[test]
    fn test_compute_deltas() {
        let before = vec![
            ("solvela_requests_total".to_string(), 100.0),
            ("solvela_errors_total".to_string(), 5.0),
        ];
        let after = vec![
            ("solvela_requests_total".to_string(), 200.0),
            ("solvela_errors_total".to_string(), 8.0),
            ("solvela_new_metric".to_string(), 10.0),
        ];

        let deltas = compute_deltas(&before, &after);
        assert_eq!(deltas.get("solvela_requests_total"), Some(&100.0));
        assert_eq!(deltas.get("solvela_errors_total"), Some(&3.0));
        // New metric defaults before-value to 0.0.
        assert_eq!(deltas.get("solvela_new_metric"), Some(&10.0));
    }

    #[test]
    fn test_compute_deltas_empty_before() {
        let before: Vec<(String, f64)> = vec![];
        let after = vec![("solvela_requests_total".to_string(), 50.0)];
        let deltas = compute_deltas(&before, &after);
        assert_eq!(deltas.get("solvela_requests_total"), Some(&50.0));
    }

    #[test]
    fn test_compute_deltas_no_change() {
        let before = vec![("solvela_requests_total".to_string(), 42.0)];
        let after = vec![("solvela_requests_total".to_string(), 42.0)];
        let deltas = compute_deltas(&before, &after);
        assert_eq!(deltas.get("solvela_requests_total"), Some(&0.0));
    }

    // --- validate_slos ---

    #[test]
    fn test_validate_slos_all_pass() {
        let m = MetricsCollector::new();
        for _ in 0..100 {
            m.record_success(std::time::Duration::from_millis(100));
        }
        let snapshot = m.snapshot();
        let thresholds = SloThresholds {
            p99_ms: 5000,
            error_rate: 0.05,
        };
        let results = validate_slos(&snapshot, &thresholds);
        assert_eq!(results.len(), 2);
        for r in &results {
            assert!(
                r.passed,
                "SLO '{}' should pass but measured={} threshold={} {}",
                r.label, r.measured, r.threshold, r.unit
            );
        }
    }

    #[test]
    fn test_validate_slos_p99_fails() {
        let m = MetricsCollector::new();
        for _ in 0..100 {
            m.record_success(std::time::Duration::from_millis(6000));
        }
        let snapshot = m.snapshot();
        let thresholds = SloThresholds {
            p99_ms: 5000,
            error_rate: 0.05,
        };
        let results = validate_slos(&snapshot, &thresholds);
        let p99_check = results.iter().find(|r| r.label == "p99 latency").unwrap();
        assert!(
            !p99_check.passed,
            "p99 SLO should fail when latency exceeds threshold"
        );
    }

    #[test]
    fn test_validate_slos_error_rate_fails() {
        let m = MetricsCollector::new();
        for _ in 0..50 {
            m.record_success(std::time::Duration::from_millis(100));
        }
        for _ in 0..50 {
            m.record_outcome(
                crate::commands::loadtest::metrics::RequestOutcome::ServerError5xx,
                std::time::Duration::from_millis(100),
            );
        }
        let snapshot = m.snapshot();
        let thresholds = SloThresholds {
            p99_ms: 5000,
            error_rate: 0.05,
        };
        let results = validate_slos(&snapshot, &thresholds);
        let err_check = results.iter().find(|r| r.label == "error rate").unwrap();
        assert!(
            !err_check.passed,
            "error rate SLO should fail at 50% error rate"
        );
        assert!(
            (err_check.measured - 50.0).abs() < 1.0,
            "measured error rate should be ~50%, got {}",
            err_check.measured
        );
    }

    #[test]
    fn test_slo_check_result_status() {
        let pass = SloCheckResult {
            label: "p99 latency".to_string(),
            passed: true,
            measured: 100.0,
            threshold: 5000.0,
            unit: "ms",
        };
        let fail = SloCheckResult {
            label: "error rate".to_string(),
            passed: false,
            measured: 10.0,
            threshold: 5.0,
            unit: "%",
        };
        assert_eq!(pass.status(), "PASS");
        assert_eq!(fail.status(), "FAIL");
    }
}
