//! `rcr recover` — discover and refund stranded escrow PDAs.
//!
//! Walks the escrow program, filters by the local wallet's pubkey, decodes each
//! on-chain account, and (optionally) submits refund transactions for any
//! escrow whose `expiry_slot` has passed.
//!
//! Dry-run mode (default) lists what would happen without sending anything.
//! `--execute` actually submits refund transactions for expired PDAs.

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};

use crate::commands::wallet::load_wallet;

/// Default mainnet escrow program ID.
const ESCROW_PROGRAM_ID: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

/// USDC mint on Solana mainnet-beta.
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// Length of Anchor account discriminator.
const DISC_LEN: usize = 8;

// Escrow account layout (anchor-serialized):
//   [8 disc][32 agent][32 provider][32 mint][8 amount]
//   [32 service_id][8 expiry_slot][1 bump]  = 153 bytes total
const AGENT_OFFSET: usize = DISC_LEN; // 8
const PROVIDER_OFFSET: usize = DISC_LEN + 32; // 40
const MINT_OFFSET: usize = DISC_LEN + 64; // 72
const AMOUNT_OFFSET: usize = DISC_LEN + 96; // 104
const SERVICE_ID_OFFSET: usize = DISC_LEN + 104; // 112
const EXPIRY_OFFSET: usize = DISC_LEN + 136; // 144
const BUMP_OFFSET: usize = DISC_LEN + 144; // 152
const ESCROW_ACCOUNT_LEN: usize = DISC_LEN + 145; // 153

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Format a μUSDC atomic amount as a decimal USDC string with 6 decimal places.
/// 1 USDC = 1_000_000 μUSDC.
fn atomic_to_usdc(atomic: u64) -> String {
    let whole = atomic / 1_000_000;
    let frac = atomic % 1_000_000;
    format!("{whole}.{frac:06}")
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Decoded on-chain escrow account with its PDA and rent balance.
#[derive(Debug, Clone)]
struct FoundEscrow {
    pda: [u8; 32],
    #[allow(dead_code)]
    agent: [u8; 32],
    #[allow(dead_code)]
    provider: [u8; 32],
    #[allow(dead_code)]
    mint: [u8; 32],
    amount: u64,
    service_id: [u8; 32],
    expiry_slot: u64,
    #[allow(dead_code)]
    bump: u8,
    lamports: u64,
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

pub async fn run(
    api_url: &str,
    execute: bool,
    yes: bool,
    program_id_override: Option<String>,
) -> Result<()> {
    let _ = api_url; // api_url is unused here — kept for signature symmetry with other commands

    // --- Load wallet ---
    let wallet = load_wallet()?;
    let private_key_b58 = wallet["private_key"]
        .as_str()
        .context("wallet missing private_key field")?;

    let key_bytes = bs58::decode(private_key_b58)
        .into_vec()
        .context("failed to decode private key from base58")?;
    if key_bytes.len() != 64 {
        return Err(anyhow!(
            "private key must be 64 bytes (seed || pubkey), got {}",
            key_bytes.len()
        ));
    }
    let seed: [u8; 32] = key_bytes[..32]
        .try_into()
        .map_err(|_| anyhow!("failed to slice seed from keypair bytes"))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let agent_pubkey: [u8; 32] = signing_key.verifying_key().to_bytes();
    let agent_pubkey_b58 = bs58::encode(agent_pubkey).into_string();

    // --- Resolve RPC URL ---
    let rpc_url = std::env::var("SOLANA_RPC_URL")
        .or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))
        .map_err(|_| {
            anyhow!(
                "SOLANA_RPC_URL required for recover. \
                 Set it to your Solana RPC endpoint (e.g. https://api.mainnet-beta.solana.com)."
            )
        })?;

    // --- Resolve program ID ---
    let program_id_str = program_id_override
        .as_deref()
        .unwrap_or(ESCROW_PROGRAM_ID)
        .to_string();

    let client = reqwest::Client::new();

    // --- Discover escrows owned by the local wallet ---
    let escrows = discover_escrows(&rpc_url, &program_id_str, &agent_pubkey, &client)
        .await
        .context("failed to discover escrow PDAs")?;

    let current_slot = crate::commands::solana_tx::fetch_current_slot(&rpc_url, &client)
        .await
        .context("failed to fetch current slot")?;

    // --- Summary header ---
    println!("Escrow recovery");
    println!("  Agent        : {agent_pubkey_b58}");
    println!("  Program      : {program_id_str}");
    println!("  Current slot : {current_slot}");
    println!("  PDAs found   : {}", escrows.len());
    println!();

    if escrows.is_empty() {
        println!("No escrow PDAs owned by this wallet.");
        return Ok(());
    }

    // --- Per-escrow table ---
    let mut expired: Vec<&FoundEscrow> = Vec::new();
    let mut total_locked: u64 = 0;
    let mut total_rent: u64 = 0;

    for esc in &escrows {
        let status = if current_slot >= esc.expiry_slot {
            expired.push(esc);
            "expired"
        } else {
            "pending"
        };
        total_locked = total_locked.saturating_add(esc.amount);
        total_rent = total_rent.saturating_add(esc.lamports);

        let pda_b58 = bs58::encode(esc.pda).into_string();
        let sid_hex: String = esc.service_id[..4]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        println!(
            "  {pda_b58}  {amount:>12} atomic ({usdc} USDC)  {status:>7}  service_id={sid_hex}…  rent={lamports} lamports  expiry_slot={expiry}",
            amount = esc.amount,
            usdc = atomic_to_usdc(esc.amount),
            lamports = esc.lamports,
            expiry = esc.expiry_slot,
        );
    }

    println!();
    println!(
        "Totals: {} escrows, {} expired, {} atomic ({} USDC) locked, {} lamports rent",
        escrows.len(),
        expired.len(),
        total_locked,
        atomic_to_usdc(total_locked),
        total_rent
    );
    println!();

    // --- Dry-run: stop here ---
    if !execute {
        println!("Dry run — re-run with --execute to submit refunds.");
        return Ok(());
    }

    // --- Execute path ---
    if expired.is_empty() {
        println!("Nothing to refund: no expired PDAs.");
        return Ok(());
    }

    // Pre-balance
    let pre_balance = fetch_sol_balance(&rpc_url, &agent_pubkey_b58, &client)
        .await
        .unwrap_or(0);
    println!("Pre-refund SOL balance : {pre_balance} lamports");

    // Confirmation prompt
    if !yes {
        print!("Submit {} refund transaction(s)? [y/N] ", expired.len());
        use std::io::Write;
        std::io::stdout().flush()?;
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !matches!(input.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    // Fetch blockhash once up-front (blockhashes live ~150 slots, plenty for a few txs).
    let recent_blockhash = crate::commands::solana_tx::fetch_blockhash(&rpc_url, &client)
        .await
        .context("failed to fetch recent blockhash")?;

    let mut success = 0usize;
    let mut failure = 0usize;

    for esc in expired {
        let pda_b58 = bs58::encode(esc.pda).into_string();
        print!("  refund {pda_b58} … ");
        use std::io::Write;
        std::io::stdout().flush().ok();

        let refund_tx =
            match x402::escrow::refund::build_refund_tx(&x402::escrow::refund::RefundParams {
                agent_keypair_b58: private_key_b58.to_string(),
                escrow_pda_b58: pda_b58.clone(),
                usdc_mint_b58: USDC_MINT.to_string(),
                escrow_program_id_b58: program_id_str.clone(),
                service_id: esc.service_id,
                recent_blockhash,
            }) {
                Ok(tx) => tx,
                Err(e) => {
                    println!("build failed: {e}");
                    failure += 1;
                    continue;
                }
            };

        match submit_refund_tx(&rpc_url, &refund_tx, &client).await {
            Ok(sig) => {
                println!("ok ({sig})");
                success += 1;
            }
            Err(e) => {
                println!("failed: {e}");
                failure += 1;
            }
        }
    }

    // Post-balance
    let post_balance = fetch_sol_balance(&rpc_url, &agent_pubkey_b58, &client)
        .await
        .unwrap_or(0);
    println!();
    println!("Post-refund SOL balance: {post_balance} lamports");
    println!("Recovery complete: {success} succeeded, {failure} failed");

    Ok(())
}

// ---------------------------------------------------------------------------
// Discovery
// ---------------------------------------------------------------------------

/// Discover all escrow PDAs owned by the given agent pubkey using the
/// program's `getProgramAccounts` endpoint with a dataSize + memcmp filter.
async fn discover_escrows(
    rpc_url: &str,
    program_id: &str,
    agent_pubkey: &[u8; 32],
    client: &reqwest::Client,
) -> Result<Vec<FoundEscrow>> {
    let agent_b58 = bs58::encode(agent_pubkey).into_string();

    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getProgramAccounts",
        "params": [
            program_id,
            {
                "encoding": "base64",
                "commitment": "confirmed",
                "filters": [
                    { "dataSize": ESCROW_ACCOUNT_LEN },
                    { "memcmp": { "offset": AGENT_OFFSET, "bytes": agent_b58 } }
                ]
            }
        ]
    });

    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC for getProgramAccounts")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Solana RPC returned HTTP {}: {}", status, text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse getProgramAccounts response")?;

    if let Some(err) = json.get("error") {
        return Err(anyhow!(
            "Solana RPC error: {}",
            serde_json::to_string(err).unwrap_or_default()
        ));
    }

    let entries = json["result"]
        .as_array()
        .ok_or_else(|| anyhow!("getProgramAccounts response missing result array"))?;

    let mut found = Vec::with_capacity(entries.len());
    for entry in entries {
        let pubkey_b58 = entry["pubkey"]
            .as_str()
            .ok_or_else(|| anyhow!("entry missing pubkey field"))?;
        let pda = x402::escrow::pda::decode_bs58_pubkey(pubkey_b58)
            .map_err(|e| anyhow!("invalid pubkey {pubkey_b58}: {e}"))?;

        let data_array = entry["account"]["data"]
            .as_array()
            .ok_or_else(|| anyhow!("entry account.data is not an array"))?;
        let data_b64 = data_array
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("entry account.data[0] missing"))?;
        let data_bytes = BASE64
            .decode(data_b64)
            .with_context(|| format!("failed to base64-decode account data for {pubkey_b58}"))?;

        let lamports = entry["account"]["lamports"].as_u64().unwrap_or(0);

        if let Some(decoded) = decode_escrow_account(pda, lamports, &data_bytes) {
            found.push(decoded);
        }
    }

    Ok(found)
}

/// Decode an escrow account's 153-byte data blob into a `FoundEscrow`.
///
/// Returns `None` if the data length does not match the expected layout.
/// The dataSize RPC filter already ensures this, but we double-check defensively.
fn decode_escrow_account(pda: [u8; 32], lamports: u64, data: &[u8]) -> Option<FoundEscrow> {
    if data.len() != ESCROW_ACCOUNT_LEN {
        return None;
    }

    let mut agent = [0u8; 32];
    agent.copy_from_slice(&data[AGENT_OFFSET..AGENT_OFFSET + 32]);

    let mut provider = [0u8; 32];
    provider.copy_from_slice(&data[PROVIDER_OFFSET..PROVIDER_OFFSET + 32]);

    let mut mint = [0u8; 32];
    mint.copy_from_slice(&data[MINT_OFFSET..MINT_OFFSET + 32]);

    let mut amount_bytes = [0u8; 8];
    amount_bytes.copy_from_slice(&data[AMOUNT_OFFSET..AMOUNT_OFFSET + 8]);
    let amount = u64::from_le_bytes(amount_bytes);

    let mut service_id = [0u8; 32];
    service_id.copy_from_slice(&data[SERVICE_ID_OFFSET..SERVICE_ID_OFFSET + 32]);

    let mut expiry_bytes = [0u8; 8];
    expiry_bytes.copy_from_slice(&data[EXPIRY_OFFSET..EXPIRY_OFFSET + 8]);
    let expiry_slot = u64::from_le_bytes(expiry_bytes);

    let bump = data[BUMP_OFFSET];

    Some(FoundEscrow {
        pda,
        agent,
        provider,
        mint,
        amount,
        service_id,
        expiry_slot,
        bump,
        lamports,
    })
}

// ---------------------------------------------------------------------------
// Refund submission
// ---------------------------------------------------------------------------

/// Submit a base64-encoded signed transaction via `sendTransaction`.
///
/// Returns the transaction signature on success.
async fn submit_refund_tx(
    rpc_url: &str,
    base64_tx: &str,
    client: &reqwest::Client,
) -> Result<String> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [
            base64_tx,
            {
                "encoding": "base64",
                "skipPreflight": false,
                "preflightCommitment": "confirmed"
            }
        ]
    });

    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC for sendTransaction")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Solana RPC returned HTTP {}: {}", status, text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse sendTransaction response")?;

    if let Some(err) = json.get("error") {
        return Err(anyhow!(
            "Solana RPC error: {}",
            serde_json::to_string(err).unwrap_or_default()
        ));
    }

    json["result"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow!("sendTransaction response missing result signature"))
}

/// Fetch a wallet's SOL balance (lamports) via `getBalance`.
async fn fetch_sol_balance(
    rpc_url: &str,
    wallet_b58: &str,
    client: &reqwest::Client,
) -> Result<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getBalance",
        "params": [wallet_b58, {"commitment": "confirmed"}]
    });
    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC for getBalance")?;
    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse getBalance response")?;
    json["result"]["value"]
        .as_u64()
        .ok_or_else(|| anyhow!("getBalance response missing result.value"))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// RAII guard that removes an env var on drop (panic-safe cleanup).
    struct EnvGuard(&'static str);
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            std::env::remove_var(self.0);
        }
    }

    /// Create a temp home with a valid wallet.
    fn setup_wallet() -> (tempfile::TempDir, [u8; 32], String) {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        std::env::set_var("HOME", tmp.path());
        let dir = tmp.path().join(".rustyclawrouter");
        std::fs::create_dir_all(&dir).expect("mkdir");

        let mut seed = [0u8; 32];
        seed[0] = 42;
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let agent_pubkey = verifying_key.to_bytes();
        let mut full_key = [0u8; 64];
        full_key[..32].copy_from_slice(&seed);
        full_key[32..].copy_from_slice(&agent_pubkey);

        let wallet = serde_json::json!({
            "private_key": bs58::encode(&full_key).into_string(),
            "address": bs58::encode(&agent_pubkey).into_string(),
            "created_at": "2026-01-01T00:00:00Z"
        });
        std::fs::write(
            dir.join("wallet.json"),
            serde_json::to_string_pretty(&wallet).expect("json"),
        )
        .expect("write wallet");

        let agent_b58 = bs58::encode(&agent_pubkey).into_string();
        (tmp, agent_pubkey, agent_b58)
    }

    /// Construct a synthetic 153-byte escrow account data blob.
    fn make_escrow_data(
        agent: &[u8; 32],
        amount: u64,
        service_id: &[u8; 32],
        expiry_slot: u64,
    ) -> Vec<u8> {
        let mut data = vec![0u8; ESCROW_ACCOUNT_LEN];
        // Discriminator (doesn't matter, we don't validate it)
        data[..DISC_LEN].copy_from_slice(&[7u8; 8]);
        data[AGENT_OFFSET..AGENT_OFFSET + 32].copy_from_slice(agent);
        data[PROVIDER_OFFSET..PROVIDER_OFFSET + 32].copy_from_slice(&[2u8; 32]);
        let mint = x402::escrow::pda::decode_bs58_pubkey(USDC_MINT).expect("valid mint");
        data[MINT_OFFSET..MINT_OFFSET + 32].copy_from_slice(&mint);
        data[AMOUNT_OFFSET..AMOUNT_OFFSET + 8].copy_from_slice(&amount.to_le_bytes());
        data[SERVICE_ID_OFFSET..SERVICE_ID_OFFSET + 32].copy_from_slice(service_id);
        data[EXPIRY_OFFSET..EXPIRY_OFFSET + 8].copy_from_slice(&expiry_slot.to_le_bytes());
        data[BUMP_OFFSET] = 255;
        data
    }

    /// Build the PDA b58 string for an agent + service_id combination.
    fn derived_pda_b58(agent: &[u8; 32], service_id: &[u8; 32]) -> String {
        let program_id =
            x402::escrow::pda::decode_bs58_pubkey(ESCROW_PROGRAM_ID).expect("valid program id");
        let (pda, _bump) =
            x402::escrow::pda::find_program_address(&[b"escrow", agent, service_id], &program_id)
                .expect("valid PDA");
        bs58::encode(pda).into_string()
    }

    // --- Pure unit tests ---

    #[test]
    fn test_atomic_to_usdc() {
        assert_eq!(atomic_to_usdc(0), "0.000000");
        assert_eq!(atomic_to_usdc(2625), "0.002625");
        assert_eq!(atomic_to_usdc(1_000_000), "1.000000");
        assert_eq!(atomic_to_usdc(7_879), "0.007879");
        assert_eq!(atomic_to_usdc(1_500_000), "1.500000");
    }

    #[test]
    fn test_decode_escrow_account_layout() {
        let agent = [0xAAu8; 32];
        let service_id = [0xBBu8; 32];
        let data = make_escrow_data(&agent, 1_234_567, &service_id, 555);
        let pda = [0xCCu8; 32];
        let decoded = decode_escrow_account(pda, 9999, &data).expect("decode should succeed");

        assert_eq!(decoded.pda, pda);
        assert_eq!(decoded.agent, agent);
        assert_eq!(decoded.amount, 1_234_567);
        assert_eq!(decoded.service_id, service_id);
        assert_eq!(decoded.expiry_slot, 555);
        assert_eq!(decoded.bump, 255);
        assert_eq!(decoded.lamports, 9999);
        // Mint should match USDC
        let usdc = x402::escrow::pda::decode_bs58_pubkey(USDC_MINT).unwrap();
        assert_eq!(decoded.mint, usdc);
    }

    #[test]
    fn test_decode_escrow_account_wrong_size() {
        let data = vec![0u8; 100];
        let result = decode_escrow_account([0u8; 32], 0, &data);
        assert!(result.is_none(), "wrong-size data should return None");
    }

    // --- Mocked async tests ---

    #[tokio::test]
    async fn test_recover_no_pdas_found() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let (_wallet_dir, _agent_pubkey, _agent_b58) = setup_wallet();

        let mock = MockServer::start().await;

        // getProgramAccounts → empty
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getProgramAccounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": []
            })))
            .mount(&mock)
            .await;

        // getSlot
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSlot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 100
            })))
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run("http://ignored", false, false, None).await;
        assert!(
            result.is_ok(),
            "recover should succeed with empty result: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_recover_list_mode_no_send() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let (_wallet_dir, agent_pubkey, _agent_b58) = setup_wallet();

        let mock = MockServer::start().await;

        // Synthetic expired escrow
        let service_id = [0x11u8; 32];
        let data = make_escrow_data(&agent_pubkey, 1_000_000, &service_id, 50);
        let pda_b58 = derived_pda_b58(&agent_pubkey, &service_id);

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getProgramAccounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": [{
                    "pubkey": pda_b58,
                    "account": {
                        "lamports": 2039280,
                        "owner": ESCROW_PROGRAM_ID,
                        "data": [BASE64.encode(&data), "base64"],
                        "executable": false,
                        "rentEpoch": 0,
                    }
                }]
            })))
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSlot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 10_000
            })))
            .mount(&mock)
            .await;

        // Fail if sendTransaction is called
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("sendTransaction"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        // execute = false
        let result = run("http://ignored", false, false, None).await;
        assert!(
            result.is_ok(),
            "list mode should succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_recover_skips_pending() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let (_wallet_dir, agent_pubkey, _agent_b58) = setup_wallet();

        let mock = MockServer::start().await;

        // Pending escrow: expiry_slot > current slot
        let service_id = [0x22u8; 32];
        let data = make_escrow_data(&agent_pubkey, 500_000, &service_id, 20_000);
        let pda_b58 = derived_pda_b58(&agent_pubkey, &service_id);

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getProgramAccounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": [{
                    "pubkey": pda_b58,
                    "account": {
                        "lamports": 2039280,
                        "owner": ESCROW_PROGRAM_ID,
                        "data": [BASE64.encode(&data), "base64"],
                        "executable": false,
                        "rentEpoch": 0,
                    }
                }]
            })))
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSlot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 10_000
            })))
            .mount(&mock)
            .await;

        // getBalance (used in execute path)
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getBalance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"value": 1_000_000}
            })))
            .mount(&mock)
            .await;

        // Must not be called
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("sendTransaction"))
            .respond_with(ResponseTemplate::new(500))
            .expect(0)
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        // execute=true, yes=true — but no PDAs are expired, so no sendTransaction
        let result = run("http://ignored", true, true, None).await;
        assert!(
            result.is_ok(),
            "execute mode with no expired PDAs should succeed: {:?}",
            result.err()
        );
    }

    #[tokio::test]
    async fn test_recover_executes_expired() {
        let _lock = crate::ENV_MUTEX.lock().await;
        let (_wallet_dir, agent_pubkey, _agent_b58) = setup_wallet();

        let mock = MockServer::start().await;

        let service_id = [0x33u8; 32];
        let data = make_escrow_data(&agent_pubkey, 2_500_000, &service_id, 50);
        let pda_b58 = derived_pda_b58(&agent_pubkey, &service_id);

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getProgramAccounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": [{
                    "pubkey": pda_b58,
                    "account": {
                        "lamports": 2039280,
                        "owner": ESCROW_PROGRAM_ID,
                        "data": [BASE64.encode(&data), "base64"],
                        "executable": false,
                        "rentEpoch": 0,
                    }
                }]
            })))
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getSlot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": 100_000
            })))
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getBalance"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"value": 5_000_000}
            })))
            .mount(&mock)
            .await;

        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("getLatestBlockhash"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "value": {
                        "blockhash": "11111111111111111111111111111111",
                        "lastValidBlockHeight": 9999
                    }
                }
            })))
            .mount(&mock)
            .await;

        // Must be called at least once
        Mock::given(method("POST"))
            .and(path("/"))
            .and(body_string_contains("sendTransaction"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": "5Kn9LvXy3T8Z2pQc4rVnYbWxJhGfM6s1uAeTdBjK7CxNhWzQmRpA"
            })))
            .expect(1..)
            .mount(&mock)
            .await;

        std::env::set_var("SOLANA_RPC_URL", mock.uri());
        let _env_guard = EnvGuard("SOLANA_RPC_URL");

        let result = run("http://ignored", true, true, None).await;
        assert!(
            result.is_ok(),
            "execute mode with expired PDA should succeed: {:?}",
            result.err()
        );
    }
}
