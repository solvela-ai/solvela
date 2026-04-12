use std::fs;
use std::io::{self, Write};

use anyhow::{Context, Result};
use serde::Serialize;

use super::config::LoadTestConfig;
use super::metrics::MetricsSnapshot;

// ANSI colour codes — avoids pulling in a crate for a handful of escape sequences.
const BOLD: &str = "\x1b[1m";
const GREEN: &str = "\x1b[32m";
const RED: &str = "\x1b[31m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// SLO validation results bundled alongside the raw snapshot.
#[derive(Debug, Clone, Serialize)]
pub struct SloResult {
    pub p99_pass: bool,
    pub error_rate_pass: bool,
    pub overall_pass: bool,
}

impl SloResult {
    fn evaluate(snapshot: &MetricsSnapshot, config: &LoadTestConfig) -> Self {
        let p99_pass = snapshot.p99_ms <= config.slo.p99_ms;
        let error_rate_pass = snapshot.error_rate() <= config.slo.error_rate;
        Self {
            p99_pass,
            error_rate_pass,
            overall_pass: p99_pass && error_rate_pass,
        }
    }
}

/// Print a human-readable load test report to stdout.
///
/// Outputs a structured terminal table with:
/// - Request outcome breakdown (success, 402, 429, 5xx, timeout, other, dropped)
/// - Latency percentiles (p50, p95, p99, max)
/// - Effective RPS and error rate
/// - SLO validation results (PASS / FAIL)
pub fn print_terminal_report(snapshot: &MetricsSnapshot, config: &LoadTestConfig) {
    let stdout = io::stdout();
    let mut out = stdout.lock();
    print_report_to(&mut out, snapshot, config);
}

fn print_report_to(out: &mut impl Write, snapshot: &MetricsSnapshot, config: &LoadTestConfig) {
    let slo = SloResult::evaluate(snapshot, config);
    let effective_rps = snapshot.effective_rps(config.duration_secs);
    let error_rate_pct = snapshot.error_rate() * 100.0;
    let errors = snapshot.total_requests.saturating_sub(snapshot.successful);

    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "{BOLD}{CYAN}╔══════════════════════════════════════════════╗{RESET}"
    );
    let _ = writeln!(
        out,
        "{BOLD}{CYAN}║          Solvela Load Test Report            ║{RESET}"
    );
    let _ = writeln!(
        out,
        "{BOLD}{CYAN}╚══════════════════════════════════════════════╝{RESET}"
    );

    // --- Configuration summary ---
    let _ = writeln!(out, "\n{BOLD}Configuration{RESET}");
    let _ = writeln!(out, "  Target:      {}", config.api_url);
    let _ = writeln!(out, "  Mode:        {:?}", config.mode);
    let _ = writeln!(
        out,
        "  Target RPS:  {}  Duration: {}s  Concurrency: {}",
        config.rps, config.duration_secs, config.concurrency
    );

    // --- Request breakdown ---
    let _ = writeln!(out, "\n{BOLD}Requests{RESET}");
    let _ = writeln!(out, "  Total:       {}", snapshot.total_requests);
    let _ = writeln!(out, "  Successful:  {GREEN}{}{RESET}", snapshot.successful);
    let _ = writeln!(out, "  Errors:      {}", errors);
    let _ = writeln!(
        out,
        "    402 (payment required): {}",
        snapshot.payment_required_402
    );
    let _ = writeln!(
        out,
        "    429 (rate limited):      {}",
        snapshot.rate_limited_429
    );
    let _ = writeln!(
        out,
        "    5xx (server error):      {}",
        snapshot.server_errors_5xx
    );
    let _ = writeln!(out, "    Timeout:                 {}", snapshot.timeouts);
    let _ = writeln!(
        out,
        "    Other:                   {}",
        snapshot.other_errors
    );
    let _ = writeln!(
        out,
        "  Dropped (backpressure): {}",
        snapshot.dropped_requests
    );

    // --- Latency ---
    let _ = writeln!(out, "\n{BOLD}Latency{RESET}");
    let _ = writeln!(out, "  p50:  {}ms", snapshot.p50_ms);
    let _ = writeln!(out, "  p95:  {}ms", snapshot.p95_ms);
    let p99_colour = if slo.p99_pass { GREEN } else { RED };
    let _ = writeln!(
        out,
        "  p99:  {p99_colour}{}ms{RESET}  (SLO ≤ {}ms)",
        snapshot.p99_ms, config.slo.p99_ms
    );
    let _ = writeln!(out, "  max:  {}ms", snapshot.max_ms);
    let _ = writeln!(out, "  mean: {}ms", snapshot.mean_ms);

    // --- Throughput ---
    let _ = writeln!(out, "\n{BOLD}Throughput{RESET}");
    let _ = writeln!(out, "  Effective RPS: {effective_rps:.2}");
    let err_colour = if slo.error_rate_pass { GREEN } else { RED };
    let _ = writeln!(
        out,
        "  Error rate:    {err_colour}{error_rate_pct:.2}%{RESET}  (SLO ≤ {:.2}%)",
        config.slo.error_rate * 100.0
    );

    // --- SLO summary ---
    let _ = writeln!(out, "\n{BOLD}SLO Results{RESET}");
    let p99_label = slo_label(slo.p99_pass);
    let err_label = slo_label(slo.error_rate_pass);
    let _ = writeln!(out, "  p99 latency:  {p99_label}");
    let _ = writeln!(out, "  Error rate:   {err_label}");

    let overall_label = if slo.overall_pass {
        format!("{BOLD}{GREEN}PASS{RESET}")
    } else {
        format!("{BOLD}{RED}FAIL{RESET}")
    };
    let _ = writeln!(out, "\n  Overall:      {overall_label}");
    let _ = writeln!(out);
}

fn slo_label(pass: bool) -> String {
    if pass {
        format!("{GREEN}PASS{RESET}")
    } else {
        format!("{RED}FAIL{RESET}")
    }
}

/// Full JSON report structure written to a file.
#[derive(Debug, Serialize)]
struct JsonReport<'a> {
    config: JsonReportConfig<'a>,
    metrics: &'a MetricsSnapshot,
    computed: JsonReportComputed,
    slo: SloResult,
}

#[derive(Debug, Serialize)]
struct JsonReportConfig<'a> {
    api_url: &'a str,
    mode: &'a str,
    rps: u64,
    duration_secs: u64,
    concurrency: usize,
    slo_p99_ms: u64,
    slo_error_rate: f64,
}

#[derive(Debug, Serialize)]
struct JsonReportComputed {
    effective_rps: f64,
    error_rate: f64,
    error_count: u64,
}

/// Write the full load test report as JSON to `path`.
///
/// Creates or truncates the file. Returns an error if the path is not writable.
pub fn write_json_report(
    snapshot: &MetricsSnapshot,
    config: &LoadTestConfig,
    path: &str,
) -> Result<()> {
    let slo = SloResult::evaluate(snapshot, config);
    let effective_rps = snapshot.effective_rps(config.duration_secs);
    let error_count = snapshot.total_requests.saturating_sub(snapshot.successful);

    let mode_str = format!("{:?}", config.mode);
    let report = JsonReport {
        config: JsonReportConfig {
            api_url: &config.api_url,
            mode: &mode_str,
            rps: config.rps,
            duration_secs: config.duration_secs,
            concurrency: config.concurrency,
            slo_p99_ms: config.slo.p99_ms,
            slo_error_rate: config.slo.error_rate,
        },
        metrics: snapshot,
        computed: JsonReportComputed {
            effective_rps,
            error_rate: snapshot.error_rate(),
            error_count,
        },
        slo,
    };

    let json = serde_json::to_string_pretty(&report).context("failed to serialize JSON report")?;

    fs::write(path, json).with_context(|| format!("failed to write JSON report to '{path}'"))?;

    // Confirm write to stderr so it doesn't pollute captured stdout in tests.
    eprintln!("{YELLOW}JSON report written to: {path}{RESET}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::loadtest::{
        config::{LoadTestMode, SloThresholds, TierWeights},
        metrics::MetricsCollector,
    };
    use std::time::Duration;

    fn make_config(p99_ms: u64, error_rate: f64) -> LoadTestConfig {
        LoadTestConfig {
            api_url: "http://localhost:8402".to_string(),
            rps: 10,
            duration_secs: 30,
            concurrency: 20,
            mode: LoadTestMode::DevBypass,
            tier_weights: TierWeights::default(),
            slo: SloThresholds { p99_ms, error_rate },
            report_json: None,
            prometheus_url: None,
            dry_run: false,
            model: "auto".to_string(),
        }
    }

    fn make_snapshot_all_success(n: u64, latency_ms: u64) -> MetricsSnapshot {
        let collector = MetricsCollector::new();
        for _ in 0..n {
            collector.record_success(Duration::from_millis(latency_ms));
        }
        collector.snapshot()
    }

    // ── SloResult ────────────────────────────────────────────────────────────

    #[test]
    fn test_slo_result_pass_when_under_thresholds() {
        let snap = make_snapshot_all_success(100, 50);
        let config = make_config(5000, 0.05);
        let slo = SloResult::evaluate(&snap, &config);
        assert!(slo.p99_pass, "p99 should pass when all latencies are low");
        assert!(slo.error_rate_pass, "error rate should pass when 0 errors");
        assert!(slo.overall_pass);
    }

    #[test]
    fn test_slo_result_fails_p99_when_latency_exceeds_threshold() {
        // Record 100 requests at 6000ms — p99 will be 6000ms, SLO is 5000ms.
        let snap = make_snapshot_all_success(100, 6000);
        let config = make_config(5000, 0.05);
        let slo = SloResult::evaluate(&snap, &config);
        assert!(
            !slo.p99_pass,
            "p99 should fail when all latencies are 6000ms > 5000ms SLO"
        );
        assert!(!slo.overall_pass);
    }

    #[test]
    fn test_slo_result_fails_error_rate_when_too_many_errors() {
        use crate::commands::loadtest::metrics::RequestOutcome;

        let collector = MetricsCollector::new();
        for _ in 0..50 {
            collector.record_success(Duration::from_millis(10));
        }
        for _ in 0..50 {
            collector.record_outcome(RequestOutcome::ServerError5xx, Duration::from_millis(10));
        }
        let snap = collector.snapshot();
        // 50% error rate, SLO is 5%.
        let config = make_config(5000, 0.05);
        let slo = SloResult::evaluate(&snap, &config);
        assert!(!slo.error_rate_pass, "50% error rate should fail 5% SLO");
        assert!(!slo.overall_pass);
    }

    #[test]
    fn test_slo_result_overall_pass_requires_both() {
        // p99 passes but error rate fails.
        use crate::commands::loadtest::metrics::RequestOutcome;

        let collector = MetricsCollector::new();
        for _ in 0..90 {
            collector.record_success(Duration::from_millis(100));
        }
        for _ in 0..10 {
            collector.record_outcome(RequestOutcome::OtherError, Duration::from_millis(100));
        }
        let snap = collector.snapshot();
        // 10% error rate, SLO is 5%.
        let config = make_config(5000, 0.05);
        let slo = SloResult::evaluate(&snap, &config);
        assert!(slo.p99_pass, "p99 should pass at 100ms");
        assert!(!slo.error_rate_pass, "10% error rate should fail 5% SLO");
        assert!(!slo.overall_pass, "overall must fail if either check fails");
    }

    // ── Terminal report ───────────────────────────────────────────────────────

    #[test]
    fn test_terminal_report_contains_key_fields() {
        let snap = make_snapshot_all_success(100, 50);
        let config = make_config(5000, 0.05);

        let mut buf = Vec::new();
        print_report_to(&mut buf, &snap, &config);
        let output = String::from_utf8(buf).expect("valid utf8");

        assert!(output.contains("100"), "should contain total request count");
        assert!(output.contains("localhost:8402"), "should contain api_url");
        assert!(output.contains("SLO"), "should contain SLO section");
        assert!(output.contains("PASS"), "all-success run should show PASS");
    }

    #[test]
    fn test_terminal_report_shows_fail_on_slo_violation() {
        let snap = make_snapshot_all_success(10, 9999);
        // SLO: p99 must be ≤ 100ms — will fail.
        let config = make_config(100, 0.05);

        let mut buf = Vec::new();
        print_report_to(&mut buf, &snap, &config);
        let output = String::from_utf8(buf).expect("valid utf8");

        assert!(output.contains("FAIL"), "high-latency run should show FAIL");
    }

    #[test]
    fn test_terminal_report_empty_snapshot() {
        let collector = MetricsCollector::new();
        let snap = collector.snapshot();
        let config = make_config(5000, 0.05);

        let mut buf = Vec::new();
        // Should not panic on zero requests.
        print_report_to(&mut buf, &snap, &config);
        let output = String::from_utf8(buf).expect("valid utf8");
        assert!(output.contains("Total"));
    }

    // ── JSON report ───────────────────────────────────────────────────────────

    #[test]
    fn test_write_json_report_creates_valid_json() {
        let snap = make_snapshot_all_success(50, 100);
        let config = make_config(5000, 0.05);

        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir
            .path()
            .join("report.json")
            .to_string_lossy()
            .into_owned();

        write_json_report(&snap, &config, &path).expect("write should succeed");

        let contents = std::fs::read_to_string(&path).expect("file readable");
        let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid JSON");

        assert_eq!(parsed["metrics"]["total_requests"], 50);
        assert_eq!(parsed["slo"]["overall_pass"], true);
        assert_eq!(parsed["config"]["rps"], 10);
        assert!(parsed["computed"]["effective_rps"].is_number());
    }

    #[test]
    fn test_write_json_report_slo_fail_encoded_correctly() {
        use crate::commands::loadtest::metrics::RequestOutcome;

        let collector = MetricsCollector::new();
        for _ in 0..50 {
            collector.record_success(Duration::from_millis(10));
        }
        for _ in 0..50 {
            collector.record_outcome(RequestOutcome::ServerError5xx, Duration::from_millis(10));
        }
        let snap = collector.snapshot();
        let config = make_config(5000, 0.05);

        let dir = tempfile::tempdir().expect("tmpdir");
        let path = dir
            .path()
            .join("report.json")
            .to_string_lossy()
            .into_owned();

        write_json_report(&snap, &config, &path).expect("write should succeed");

        let contents = std::fs::read_to_string(&path).expect("file readable");
        let parsed: serde_json::Value = serde_json::from_str(&contents).expect("valid JSON");

        assert_eq!(parsed["slo"]["overall_pass"], false);
        assert_eq!(parsed["slo"]["error_rate_pass"], false);
        assert_eq!(parsed["computed"]["error_count"], 50);
    }

    #[test]
    fn test_write_json_report_bad_path_returns_error() {
        let snap = make_snapshot_all_success(1, 10);
        let config = make_config(5000, 0.05);

        let result = write_json_report(&snap, &config, "/nonexistent/dir/report.json");
        assert!(result.is_err(), "writing to a bad path should fail");
    }
}
