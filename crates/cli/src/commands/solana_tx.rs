//! USDC-SPL TransferChecked transaction builder for the CLI.
//!
//! Constructs and signs a Solana legacy transaction that transfers USDC-SPL
//! from the payer's ATA to the recipient's ATA, using the x402 crate's
//! lightweight Solana types to avoid the heavy `solana-sdk` dependency chain
//! (openssl-sys, etc.).
//!
//! Wire format produced:
//!   compact-u16(1)              -- signature count
//!   64 bytes                    -- ed25519 signature
//!   0x01                        -- header: 1 required signer
//!   0x00                        -- header: 0 readonly signed
//!   0x02                        -- header: 2 readonly unsigned (mint + token program)
//!   compact-u16(N)              -- account key count
//!   N × 32 bytes                -- account keys
//!   32 bytes                    -- recent blockhash
//!   compact-u16(1)              -- instruction count
//!   1 byte                      -- program_id_index
//!   compact-u16(M)              -- account indices length
//!   M bytes                     -- account indices
//!   compact-u16(D)              -- data length
//!   D bytes                     -- instruction data (TransferChecked)

use anyhow::{anyhow, Context, Result};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use x402::solana_types::{derive_ata, Pubkey};

/// USDC mint on Solana mainnet-beta.
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

/// USDC decimals (6).
const USDC_DECIMALS: u8 = 6;

/// Fetch the latest blockhash from a Solana JSON-RPC endpoint.
///
/// Returns the blockhash as 32 raw bytes.
async fn fetch_blockhash(rpc_url: &str, client: &reqwest::Client) -> Result<[u8; 32]> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{"commitment": "confirmed"}]
    });

    let resp = client
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .context("failed to connect to Solana RPC")?;

    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(anyhow!("Solana RPC returned HTTP {}: {}", status, text));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .context("failed to parse Solana RPC response")?;

    if let Some(err) = json.get("error") {
        return Err(anyhow!(
            "Solana RPC error: {}",
            serde_json::to_string(err).unwrap_or_default()
        ));
    }

    let blockhash_b58 = json["result"]["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| anyhow!("Solana RPC response missing blockhash field"))?;

    let bytes = bs58::decode(blockhash_b58)
        .into_vec()
        .context("failed to decode blockhash from base58")?;

    if bytes.len() != 32 {
        return Err(anyhow!(
            "blockhash has unexpected length {}, expected 32",
            bytes.len()
        ));
    }

    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Encode a usize as Solana compact-u16 wire format (1–3 bytes).
fn encode_compact_u16(mut value: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(3);
    loop {
        let mut byte = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if value == 0 {
            break;
        }
    }
    out
}

/// Build a `TransferChecked` instruction data payload.
///
/// Layout: [12, amount(8 LE), decimals(1)] — 10 bytes total.
fn transfer_checked_data(amount: u64, decimals: u8) -> [u8; 10] {
    let mut data = [0u8; 10];
    data[0] = 12; // TransferChecked discriminator
    data[1..9].copy_from_slice(&amount.to_le_bytes());
    data[9] = decimals;
    data
}

/// Serialise the legacy message body into bytes (without the version prefix).
///
/// Layout:
///   header (3 bytes)
///   compact-u16 account count + N×32 byte keys
///   32-byte recent blockhash
///   compact-u16 instruction count + instruction bytes
fn build_message(
    account_keys: &[Pubkey],
    header: [u8; 3],
    recent_blockhash: &[u8; 32],
    instructions: &[(u8, Vec<u8>, Vec<u8>)], // (program_id_index, accounts, data)
) -> Vec<u8> {
    let mut msg = Vec::new();

    // Header
    msg.extend_from_slice(&header);

    // Account keys
    msg.extend_from_slice(&encode_compact_u16(account_keys.len()));
    for key in account_keys {
        msg.extend_from_slice(&key.0);
    }

    // Recent blockhash
    msg.extend_from_slice(recent_blockhash);

    // Instructions
    msg.extend_from_slice(&encode_compact_u16(instructions.len()));
    for (program_id_index, accounts, data) in instructions {
        msg.push(*program_id_index);
        msg.extend_from_slice(&encode_compact_u16(accounts.len()));
        msg.extend_from_slice(accounts);
        msg.extend_from_slice(&encode_compact_u16(data.len()));
        msg.extend_from_slice(data);
    }

    msg
}

/// Build and sign a USDC-SPL TransferChecked transaction.
///
/// Returns the transaction as a base64-encoded Solana legacy transaction in
/// native wire format (same format that the x402 gateway verifies).
///
/// # Arguments
/// * `payer_keypair_b58` — 64-byte Solana keypair in base58 (seed || pubkey)
/// * `recipient_wallet`  — recipient's **wallet** address (not their ATA)
/// * `amount`            — transfer amount in USDC atomic units (1 USDC = 1_000_000)
/// * `rpc_url`           — Solana JSON-RPC endpoint URL
/// * `client`            — reqwest HTTP client (shared with gateway calls)
pub async fn build_usdc_transfer(
    payer_keypair_b58: &str,
    recipient_wallet: &str,
    amount: u64,
    rpc_url: &str,
    client: &reqwest::Client,
) -> Result<String> {
    if amount == 0 {
        return Err(anyhow!("transfer amount must be greater than zero"));
    }

    // --- 1. Decode keypair ---
    let key_bytes = bs58::decode(payer_keypair_b58)
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
    let verifying_key = signing_key.verifying_key();

    // Validate that the derived pubkey matches bytes 32-63 of the keypair.
    let expected_pubkey = &key_bytes[32..];
    if verifying_key.as_bytes() != expected_pubkey {
        return Err(anyhow!(
            "keypair is corrupt: derived pubkey does not match stored pubkey"
        ));
    }

    let payer_pubkey = Pubkey(verifying_key.to_bytes());

    // --- 2. Parse addresses ---
    let recipient_pubkey: Pubkey = recipient_wallet
        .parse()
        .context("invalid recipient wallet address")?;

    let usdc_mint: Pubkey = USDC_MINT.parse().expect("USDC_MINT constant is valid");
    let token_program = Pubkey::TOKEN_PROGRAM_ID;

    // --- 3. Derive ATAs ---
    let payer_ata = derive_ata(&payer_pubkey, &usdc_mint, &token_program)
        .ok_or_else(|| anyhow!("failed to derive payer ATA"))?;

    let recipient_ata = derive_ata(&recipient_pubkey, &usdc_mint, &token_program)
        .ok_or_else(|| anyhow!("failed to derive recipient ATA"))?;

    // --- 4. Fetch blockhash ---
    let recent_blockhash = fetch_blockhash(rpc_url, client).await?;

    // --- 5. Build account keys list ---
    // Accounts for TransferChecked: [source, mint, destination, authority]
    // Ordering by signer/writable status (Solana convention):
    //   index 0: payer (signer, writable)
    //   index 1: payer_ata (writable)
    //   index 2: recipient_ata (writable)
    //   index 3: usdc_mint (readonly)
    //   index 4: token_program (readonly, program)
    let account_keys = vec![
        payer_pubkey,  // 0: signer + writable (fee payer / authority)
        payer_ata,     // 1: writable (source token account)
        recipient_ata, // 2: writable (destination token account)
        usdc_mint,     // 3: readonly (mint for TransferChecked)
        token_program, // 4: readonly (SPL Token program)
    ];

    // Message header: [num_required_signatures, num_readonly_signed, num_readonly_unsigned]
    // 1 signer (payer), 0 readonly signers, 2 readonly unsigned (mint + token program)
    let header: [u8; 3] = [1, 0, 2];

    // TransferChecked account indices: [source=1, mint=3, destination=2, authority=0]
    let ix_accounts: Vec<u8> = vec![1, 3, 2, 0];
    let ix_data = transfer_checked_data(amount, USDC_DECIMALS).to_vec();
    let program_id_index: u8 = 4; // token_program is at index 4

    let instructions = vec![(program_id_index, ix_accounts, ix_data)];

    // --- 6. Serialise message ---
    let message_bytes = build_message(&account_keys, header, &recent_blockhash, &instructions);

    // --- 7. Sign ---
    use ed25519_dalek::Signer as _;
    let signature = signing_key.sign(&message_bytes);
    let sig_bytes = signature.to_bytes(); // 64 bytes

    // --- 8. Serialise as Solana wire format ---
    // compact-u16(1) + 64-byte signature + message bytes
    let mut tx_bytes = Vec::new();
    tx_bytes.extend_from_slice(&encode_compact_u16(1)); // 1 signature
    tx_bytes.extend_from_slice(&sig_bytes);
    tx_bytes.extend_from_slice(&message_bytes);

    Ok(BASE64.encode(&tx_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_keypair_b58() -> String {
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        bs58::encode(&full).into_string()
    }

    const RECIPIENT: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

    #[test]
    fn test_zero_amount_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let err = rt
            .block_on(build_usdc_transfer(
                &make_keypair_b58(),
                RECIPIENT,
                0,
                "http://localhost:8899",
                &client,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("greater than zero"),
            "expected zero-amount error, got: {err}"
        );
    }

    #[test]
    fn test_rpc_unreachable_returns_error() {
        // Bind and immediately drop to get a refused port.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let dead_rpc = format!("http://127.0.0.1:{port}");

        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let err = rt
            .block_on(build_usdc_transfer(
                &make_keypair_b58(),
                RECIPIENT,
                1_000_000,
                &dead_rpc,
                &client,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("connect") || err.to_string().contains("RPC"),
            "expected connection error, got: {err}"
        );
    }

    #[test]
    fn test_invalid_keypair_rejected() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        // Only 32 bytes — not a valid 64-byte Solana keypair
        let short_key = bs58::encode(&[0u8; 32]).into_string();
        let err = rt
            .block_on(build_usdc_transfer(
                &short_key,
                RECIPIENT,
                1_000_000,
                "http://localhost:8899",
                &client,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("64 bytes"),
            "expected key length error, got: {err}"
        );
    }

    #[test]
    fn test_corrupt_keypair_rejected() {
        // Build a 64-byte array where bytes 32-63 don't match the derived pubkey.
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        // Corrupt the stored pubkey (flip one byte).
        full[32] ^= 0xFF;
        let corrupt_keypair_b58 = bs58::encode(&full).into_string();

        let rt = tokio::runtime::Runtime::new().unwrap();
        let client = reqwest::Client::new();
        let err = rt
            .block_on(build_usdc_transfer(
                &corrupt_keypair_b58,
                RECIPIENT,
                1_000_000,
                "http://localhost:8899",
                &client,
            ))
            .unwrap_err();
        assert!(
            err.to_string().contains("corrupt"),
            "expected corrupt keypair error, got: {err}"
        );
    }

    #[test]
    fn test_encode_compact_u16_values() {
        assert_eq!(encode_compact_u16(0), vec![0x00]);
        assert_eq!(encode_compact_u16(1), vec![0x01]);
        assert_eq!(encode_compact_u16(127), vec![0x7F]);
        assert_eq!(encode_compact_u16(128), vec![0x80, 0x01]);
        assert_eq!(encode_compact_u16(255), vec![0xFF, 0x01]);
        assert_eq!(encode_compact_u16(256), vec![0x80, 0x02]);
    }

    #[test]
    fn test_transfer_checked_data_layout() {
        let data = transfer_checked_data(1_000_000, 6);
        assert_eq!(data[0], 12); // discriminator
        assert_eq!(&data[1..9], &1_000_000u64.to_le_bytes());
        assert_eq!(data[9], 6); // decimals
    }

    #[test]
    fn test_x402_escrow_pda_exports_accessible() {
        // Verify that x402 escrow PDA helpers are publicly accessible
        let program_id = x402::escrow::pda::decode_bs58_pubkey(
            "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
        ).unwrap();
        let agent = [1u8; 32];
        let service_id = [2u8; 32];
        let result = x402::escrow::pda::find_program_address(
            &[b"escrow", &agent, &service_id],
            &program_id,
        );
        assert!(result.is_some());
    }
}
