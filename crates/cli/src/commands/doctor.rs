//! `solvela doctor` — self-diagnostic checks for the CLI and gateway.
//!
//! Each check classifies its outcome as `Ok`, `Warn`, or `Error` via pure
//! [`Status`] helpers (these are exercised in unit tests). Network calls are
//! kept as thin wrappers so the classification logic stays test-friendly.

use std::time::{Duration, Instant};

use anyhow::Result;
use solvela_x402::solana_types::{derive_ata, Pubkey};

use crate::commands::util::{resolve_rpc_url, wallet_file_path};

/// USDC-SPL mint address (mainnet-beta).
const USDC_MINT_MAINNET: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// USDC-SPL mint address (devnet circle test mint).
const USDC_MINT_DEVNET: &str = "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU";

/// Latency threshold above which RPC reachability becomes a warning.
const RPC_SLOW_THRESHOLD_MS: u128 = 500;

/// LLM provider env var names checked by the gateway at boot.
const PROVIDER_ENV_VARS: &[&str] = &[
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "GOOGLE_API_KEY",
    "XAI_API_KEY",
    "DEEPSEEK_API_KEY",
];

// ---------------------------------------------------------------------------
// Pure status helpers (unit-tested)
// ---------------------------------------------------------------------------

/// Outcome of a single diagnostic check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    Warn,
    Error,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Status::Ok => "[OK]",
            Status::Warn => "[WARN]",
            Status::Error => "[ERROR]",
        }
    }

    /// ANSI-colored label for terminal output. Falls back to plain when
    /// stdout is not a terminal.
    fn colored_label(self) -> String {
        // Bright green / yellow / red on terminals; otherwise plain.
        // No external `colored` dep — keep this simple to avoid extra crates.
        let color = match self {
            Status::Ok => "\x1b[32m",
            Status::Warn => "\x1b[33m",
            Status::Error => "\x1b[31m",
        };
        if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
            format!("{color}{}\x1b[0m", self.label())
        } else {
            self.label().to_string()
        }
    }
}

/// Print a single check line: `[OK] / [WARN] / [ERROR] <message>`.
fn print_check(status: Status, message: &str) {
    println!("{} {message}", status.colored_label());
}

/// Classify provider key presence: `Ok` if any are set, `Warn` otherwise.
pub(crate) fn classify_provider_keys(env_present: &[bool]) -> Status {
    if env_present.iter().any(|&v| v) {
        Status::Ok
    } else {
        Status::Warn
    }
}

/// Classify Solana RPC reachability based on a timed result.
///
/// * `Ok` if the request succeeded under the slow threshold
/// * `Warn` if it succeeded but was slow
/// * `Error` if the request failed entirely
pub(crate) fn classify_rpc(reachable: bool, latency_ms: u128) -> Status {
    if !reachable {
        return Status::Error;
    }
    if latency_ms > RPC_SLOW_THRESHOLD_MS {
        Status::Warn
    } else {
        Status::Ok
    }
}

/// Classify a USDC token balance: zero balance is a warn (cannot pay yet).
pub(crate) fn classify_usdc_balance(amount_atomic: u64) -> Status {
    if amount_atomic == 0 {
        Status::Warn
    } else {
        Status::Ok
    }
}

/// Classify the `SOLVELA_DEMO_MODE` toggle relative to provider key presence.
///
/// This is informational only — demo mode is always considered "OK" because
/// it auto-activates at gateway startup whenever no providers are configured.
/// The richer semantics live in the printed message rather than the status.
pub(crate) fn classify_demo_mode(_demo_set: bool, _any_provider: bool) -> Status {
    Status::Ok
}

/// Classify the configured recipient wallet — must be a valid Solana pubkey.
pub(crate) fn classify_recipient_wallet(value: Option<&str>) -> Status {
    match value {
        Some(v) if !v.trim().is_empty() => match v.parse::<Pubkey>() {
            Ok(_) => Status::Ok,
            Err(_) => Status::Error,
        },
        _ => Status::Warn,
    }
}

// ---------------------------------------------------------------------------
// Network helpers (thin wrappers — not unit-tested directly)
// ---------------------------------------------------------------------------

/// JSON-RPC `getHealth` against a Solana RPC. Returns elapsed wall time.
async fn solana_rpc_health(rpc_url: &str, client: &reqwest::Client) -> (bool, Duration) {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getHealth",
    });
    let start = Instant::now();
    let result = client.post(rpc_url).json(&body).send().await;
    let elapsed = start.elapsed();
    let ok = matches!(result, Ok(r) if r.status().is_success());
    (ok, elapsed)
}

/// JSON-RPC `getTokenAccountBalance` for a wallet's USDC ATA.
///
/// Returns `Ok(None)` when the ATA does not exist, `Ok(Some(amount))`
/// when found, and `Err` on transport/parse failure.
async fn fetch_usdc_balance(
    rpc_url: &str,
    wallet_pubkey: &Pubkey,
    usdc_mint: &Pubkey,
    client: &reqwest::Client,
) -> Result<Option<u64>> {
    let ata = derive_ata(wallet_pubkey, usdc_mint, &Pubkey::TOKEN_PROGRAM_ID)
        .ok_or_else(|| anyhow::anyhow!("failed to derive USDC ATA"))?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getTokenAccountBalance",
        "params": [ata.to_string(), {"commitment": "confirmed"}]
    });
    let resp = client.post(rpc_url).json(&body).send().await?;
    let json: serde_json::Value = resp.json().await?;
    if let Some(err) = json.get("error") {
        // RPC explicitly returns an error — usually because the ATA does not
        // exist yet. Treat as zero balance rather than fatal.
        let err_str = err.to_string();
        if err_str.contains("could not find account")
            || err_str.contains("Invalid param")
            || err_str.contains("-32602")
        {
            return Ok(None);
        }
        return Err(anyhow::anyhow!("RPC error: {err_str}"));
    }
    let amount = json["result"]["value"]["amount"]
        .as_str()
        .and_then(|s| s.parse::<u64>().ok());
    Ok(amount)
}

/// Pick the USDC mint based on whether the RPC URL targets devnet.
fn usdc_mint_for_rpc(rpc_url: &str) -> &'static str {
    if rpc_url.contains("devnet") {
        USDC_MINT_DEVNET
    } else {
        USDC_MINT_MAINNET
    }
}

// ---------------------------------------------------------------------------
// Main entrypoint
// ---------------------------------------------------------------------------

pub async fn run(api_url: &str) -> Result<()> {
    println!("Running Solvela diagnostics...\n");

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()?;

    // 1. Gateway connectivity
    match client.get(format!("{api_url}/health")).send().await {
        Ok(resp) if resp.status().is_success() => {
            print_check(Status::Ok, &format!("Gateway is reachable at {api_url}"))
        }
        Ok(resp) => print_check(
            Status::Error,
            &format!("Gateway returned HTTP {} from {api_url}", resp.status()),
        ),
        Err(e) => print_check(
            Status::Error,
            &format!("Gateway unreachable at {api_url}: {e}"),
        ),
    }

    // 2. Wallet file
    let wallet_path = wallet_file_path();
    if wallet_path.exists() {
        print_check(
            Status::Ok,
            &format!("Wallet present at {}", wallet_path.display()),
        );
    } else {
        print_check(
            Status::Warn,
            &format!(
                "No wallet at {} — run `solvela wallet init`",
                wallet_path.display()
            ),
        );
    }

    // 3. Models endpoint
    match client.get(format!("{api_url}/v1/models")).send().await {
        Ok(resp) if resp.status().is_success() => {
            let body: serde_json::Value = resp.json().await.unwrap_or_default();
            let count = body["data"].as_array().map(|a| a.len()).unwrap_or(0);
            print_check(
                Status::Ok,
                &format!("Models endpoint returned {count} model(s)"),
            );
        }
        _ => print_check(Status::Warn, "Models endpoint unavailable"),
    }

    // 4. SOLANA_WALLET_KEY env var
    if std::env::var("SOLANA_WALLET_KEY").is_ok() {
        print_check(Status::Ok, "SOLANA_WALLET_KEY is set");
    } else {
        print_check(
            Status::Warn,
            "SOLANA_WALLET_KEY is not set (only required for env-based signing)",
        );
    }

    // 5. Provider API key presence
    let env_present: Vec<bool> = PROVIDER_ENV_VARS
        .iter()
        .map(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
        .collect();
    let provider_status = classify_provider_keys(&env_present);
    let configured_count = env_present.iter().filter(|p| **p).count();
    let total = PROVIDER_ENV_VARS.len();
    let any_provider = configured_count > 0;
    if any_provider {
        print_check(
            provider_status,
            &format!("{configured_count}/{total} LLM provider API keys configured"),
        );
    } else {
        print_check(
            provider_status,
            "No LLM provider API keys set — gateway will fall back to demo mode",
        );
    }

    // 6. Solana RPC reachable + latency
    let rpc_url = resolve_rpc_url();
    let (reachable, elapsed) = solana_rpc_health(&rpc_url, &client).await;
    let latency_ms = elapsed.as_millis();
    let rpc_status = classify_rpc(reachable, latency_ms);
    if reachable {
        print_check(
            rpc_status,
            &format!("Solana RPC {rpc_url} healthy ({latency_ms} ms)"),
        );
    } else {
        print_check(
            rpc_status,
            &format!("Solana RPC {rpc_url} unreachable ({latency_ms} ms)"),
        );
    }

    // 7. Wallet USDC-SPL balance (only if wallet file is loadable)
    if wallet_path.exists() {
        match crate::commands::wallet::load_wallet() {
            Ok(wallet) => {
                let address = wallet["address"].as_str().unwrap_or("");
                match address.parse::<Pubkey>() {
                    Ok(wallet_pubkey) => {
                        let mint_str = usdc_mint_for_rpc(&rpc_url);
                        // Safe: hardcoded constants are valid base58 pubkeys
                        let usdc_mint = mint_str
                            .parse::<Pubkey>()
                            .expect("hardcoded USDC mint constant");
                        match fetch_usdc_balance(&rpc_url, &wallet_pubkey, &usdc_mint, &client)
                            .await
                        {
                            Ok(Some(amount)) => {
                                let status = classify_usdc_balance(amount);
                                let usdc = (amount as f64) / 1_000_000.0;
                                print_check(
                                    status,
                                    &format!(
                                        "Wallet USDC balance: {usdc:.6} USDC ({amount} atomic)"
                                    ),
                                );
                            }
                            Ok(None) => print_check(
                                Status::Warn,
                                "Wallet has no USDC token account yet (balance: 0)",
                            ),
                            Err(e) => print_check(
                                Status::Warn,
                                &format!("Could not fetch USDC balance: {e}"),
                            ),
                        }
                    }
                    Err(e) => print_check(
                        Status::Error,
                        &format!("Wallet address is not a valid Solana pubkey: {e}"),
                    ),
                }
            }
            Err(e) => print_check(
                Status::Warn,
                &format!("Could not load wallet for balance check: {e}"),
            ),
        }
    }

    // 8. Demo mode status
    let demo_set = std::env::var("SOLVELA_DEMO_MODE").is_ok();
    let demo_status = classify_demo_mode(demo_set, any_provider);
    let demo_msg = match (demo_set, any_provider) {
        (true, _) => "SOLVELA_DEMO_MODE is set explicitly".to_string(),
        (false, true) => {
            "SOLVELA_DEMO_MODE not set — real providers configured, demo will not activate"
                .to_string()
        }
        (false, false) => {
            "SOLVELA_DEMO_MODE not set — demo will auto-activate (no providers configured)"
                .to_string()
        }
    };
    print_check(demo_status, &demo_msg);

    // 9. Recipient wallet configured
    let recipient = std::env::var("SOLVELA_SOLANA_RECIPIENT_WALLET").ok();
    let recipient_status = classify_recipient_wallet(recipient.as_deref());
    let recipient_msg = match recipient.as_deref() {
        Some(addr) if !addr.trim().is_empty() => match recipient_status {
            Status::Ok => format!("SOLVELA_SOLANA_RECIPIENT_WALLET = {addr}"),
            _ => format!("SOLVELA_SOLANA_RECIPIENT_WALLET = {addr} (invalid pubkey)"),
        },
        _ => "SOLVELA_SOLANA_RECIPIENT_WALLET not set (gateway needs this to receive payments)"
            .to_string(),
    };
    print_check(recipient_status, &recipient_msg);

    println!("\nDiagnostics complete.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Bind a TCP listener to get an OS-assigned port, then drop it.
    /// The returned URL will be connection-refused immediately (ECONNREFUSED).
    fn dead_url() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        drop(listener);
        format!("http://127.0.0.1:{port}")
    }

    // --- Status classifier unit tests ---

    #[test]
    fn classify_provider_keys_warn_when_none() {
        let result = classify_provider_keys(&[false, false, false, false, false]);
        assert_eq!(result, Status::Warn);
    }

    #[test]
    fn classify_provider_keys_ok_when_at_least_one() {
        let result = classify_provider_keys(&[false, true, false, false, false]);
        assert_eq!(result, Status::Ok);
    }

    #[test]
    fn classify_provider_keys_ok_when_all_set() {
        let result = classify_provider_keys(&[true, true, true, true, true]);
        assert_eq!(result, Status::Ok);
    }

    #[test]
    fn classify_rpc_error_when_unreachable() {
        let result = classify_rpc(false, 0);
        assert_eq!(result, Status::Error);
    }

    #[test]
    fn classify_rpc_ok_when_fast() {
        let result = classify_rpc(true, 100);
        assert_eq!(result, Status::Ok);
    }

    #[test]
    fn classify_rpc_warn_when_slow() {
        let result = classify_rpc(true, 600);
        assert_eq!(result, Status::Warn);
    }

    #[test]
    fn classify_rpc_ok_at_exact_threshold() {
        let result = classify_rpc(true, RPC_SLOW_THRESHOLD_MS);
        assert_eq!(result, Status::Ok);
    }

    #[test]
    fn classify_usdc_balance_warn_at_zero() {
        assert_eq!(classify_usdc_balance(0), Status::Warn);
    }

    #[test]
    fn classify_usdc_balance_ok_when_funded() {
        assert_eq!(classify_usdc_balance(1_000_000), Status::Ok);
    }

    #[test]
    fn classify_recipient_wallet_warn_when_unset() {
        assert_eq!(classify_recipient_wallet(None), Status::Warn);
        assert_eq!(classify_recipient_wallet(Some("")), Status::Warn);
        assert_eq!(classify_recipient_wallet(Some("   ")), Status::Warn);
    }

    #[test]
    fn classify_recipient_wallet_ok_when_valid_pubkey() {
        let result =
            classify_recipient_wallet(Some("9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM"));
        assert_eq!(result, Status::Ok);
    }

    #[test]
    fn classify_recipient_wallet_error_when_invalid() {
        assert_eq!(
            classify_recipient_wallet(Some("not-a-pubkey")),
            Status::Error
        );
        assert_eq!(classify_recipient_wallet(Some("xx")), Status::Error);
    }

    #[test]
    fn classify_demo_mode_ok_when_explicit() {
        assert_eq!(classify_demo_mode(true, true), Status::Ok);
        assert_eq!(classify_demo_mode(true, false), Status::Ok);
    }

    #[test]
    fn classify_demo_mode_ok_when_implicit_with_no_provider() {
        assert_eq!(classify_demo_mode(false, false), Status::Ok);
        assert_eq!(classify_demo_mode(false, true), Status::Ok);
    }

    #[test]
    fn usdc_mint_picks_devnet_for_devnet_url() {
        assert_eq!(
            usdc_mint_for_rpc("https://api.devnet.solana.com"),
            USDC_MINT_DEVNET
        );
    }

    #[test]
    fn usdc_mint_picks_mainnet_for_mainnet_url() {
        assert_eq!(
            usdc_mint_for_rpc("https://api.mainnet-beta.solana.com"),
            USDC_MINT_MAINNET
        );
    }

    #[test]
    fn status_label_strings_are_stable() {
        assert_eq!(Status::Ok.label(), "[OK]");
        assert_eq!(Status::Warn.label(), "[WARN]");
        assert_eq!(Status::Error.label(), "[ERROR]");
    }

    // --- Integration-style smoke tests (preserved from the original) ---

    #[tokio::test]
    async fn test_doctor_all_checks_pass() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());
        let wallet_dir = tmp.path().join(".solvela");
        std::fs::create_dir_all(&wallet_dir).expect("mkdir");
        std::fs::write(
            wallet_dir.join("wallet.json"),
            r#"{"address":"9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM","private_key":"test"}"#,
        )
        .expect("write wallet");

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [{"id": "test-model"}]
            })))
            .mount(&mock)
            .await;

        let result = run(&mock.uri()).await;
        assert!(
            result.is_ok(),
            "doctor should succeed even with mixed results"
        );
        drop(tmp);
    }

    #[tokio::test]
    async fn test_doctor_gateway_unreachable() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let result = run(&dead_url()).await;
        assert!(
            result.is_ok(),
            "doctor should not error when gateway is unreachable"
        );
    }

    #[tokio::test]
    async fn test_doctor_no_wallet() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"status": "ok"})),
            )
            .mount(&mock)
            .await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
            .mount(&mock)
            .await;

        let result = run(&mock.uri()).await;
        assert!(
            result.is_ok(),
            "doctor should handle missing wallet gracefully"
        );
    }
}
