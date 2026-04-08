//! Escrow deposit transaction builder for client SDKs.
//!
//! Builds a signed Solana legacy transaction that calls the escrow program's
//! `deposit` instruction. The transaction is returned as a base64-encoded string
//! suitable for submission to any Solana RPC endpoint.

use super::pda::{
    anchor_discriminator, decode_bs58_pubkey, derive_ata_address, find_program_address,
    ATA_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parameters required to build an escrow deposit transaction.
pub struct DepositParams {
    /// Base58-encoded 64-byte ed25519 keypair (32-byte secret || 32-byte pubkey).
    pub agent_keypair_b58: String,
    /// Base58-encoded provider wallet pubkey.
    pub provider_wallet_b58: String,
    /// Base58-encoded USDC mint pubkey.
    pub usdc_mint_b58: String,
    /// Base58-encoded escrow program ID.
    pub escrow_program_id_b58: String,
    /// Amount to deposit in atomic USDC units (must be > 0).
    pub amount: u64,
    /// 32-byte service identifier that seeds the escrow PDA.
    pub service_id: [u8; 32],
    /// Slot at which the escrow deposit expires (passed to the on-chain instruction).
    pub expiry_slot: u64,
    /// Recent blockhash (32 bytes) from `getLatestBlockhash`.
    pub recent_blockhash: [u8; 32],
}

// ---------------------------------------------------------------------------
// Transaction builder
// ---------------------------------------------------------------------------

/// Build a signed escrow deposit transaction.
///
/// Returns a base64-encoded wire-format Solana transaction ready for
/// submission via `sendTransaction`.
///
/// # Errors
///
/// Returns `Err(String)` if:
/// - `amount` is zero
/// - Any address fails to decode from base58
/// - The keypair is invalid or the derived pubkey does not match the stored one
/// - PDA or ATA derivation fails
pub fn build_deposit_tx(params: &DepositParams) -> Result<String, String> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    // Step 1: Validate amount
    if params.amount == 0 {
        return Err("deposit amount must be non-zero".to_string());
    }

    // Step 2: Decode the 64-byte keypair and validate derived pubkey
    let keypair_bytes = bs58::decode(&params.agent_keypair_b58)
        .into_vec()
        .map_err(|e| format!("invalid base58 keypair: {e}"))?;
    if keypair_bytes.len() != 64 {
        return Err(format!(
            "keypair must be 64 bytes, got {}",
            keypair_bytes.len()
        ));
    }
    let mut keypair_arr = [0u8; 64];
    keypair_arr.copy_from_slice(&keypair_bytes);

    let signing_key = SigningKey::from_keypair_bytes(&keypair_arr)
        .map_err(|e| format!("invalid keypair: {e}"))?;
    let agent_pubkey = signing_key.verifying_key().to_bytes();

    // Validate that the stored pubkey in bytes 32..64 matches the derived one
    let stored_pubkey = &keypair_bytes[32..64];
    if stored_pubkey != agent_pubkey {
        return Err("keypair pubkey mismatch: derived pubkey does not match stored pubkey".to_string());
    }

    // Step 3: Parse all addresses
    let provider_pubkey = decode_bs58_pubkey(&params.provider_wallet_b58)
        .map_err(|e| format!("provider_wallet: {e}"))?;
    let usdc_mint = decode_bs58_pubkey(&params.usdc_mint_b58)
        .map_err(|e| format!("usdc_mint: {e}"))?;
    let escrow_program_id = decode_bs58_pubkey(&params.escrow_program_id_b58)
        .map_err(|e| format!("escrow_program_id: {e}"))?;
    let token_program = decode_bs58_pubkey(TOKEN_PROGRAM_ID)
        .map_err(|e| format!("token_program: {e}"))?;
    let ata_program = decode_bs58_pubkey(ATA_PROGRAM_ID)
        .map_err(|e| format!("ata_program: {e}"))?;
    let system_program = decode_bs58_pubkey(SYSTEM_PROGRAM_ID)
        .map_err(|e| format!("system_program: {e}"))?;

    // Step 4: Derive escrow PDA
    let (escrow_pda, _bump) = find_program_address(
        &[b"escrow", &agent_pubkey, &params.service_id],
        &escrow_program_id,
    )
    .ok_or_else(|| "failed to derive escrow PDA".to_string())?;

    // Step 5: Derive agent ATA and vault ATA
    let agent_ata = derive_ata_address(&agent_pubkey, &usdc_mint)
        .ok_or_else(|| "failed to derive agent ATA".to_string())?;
    let vault_ata = derive_ata_address(&escrow_pda, &usdc_mint)
        .ok_or_else(|| "failed to derive vault ATA".to_string())?;

    // Step 6: Build account keys in the exact order matching the on-chain Deposit accounts struct
    // 0: agent_pubkey        (signer, writable)
    // 1: provider            (readonly)
    // 2: usdc_mint           (readonly)
    // 3: escrow_pda          (writable)
    // 4: agent_ata           (writable)
    // 5: vault_ata           (writable)
    // 6: token_program       (readonly)
    // 7: ata_program         (readonly)
    // 8: system_program      (readonly)
    // 9: escrow_program_id   (program invoked — appended last)
    let accounts: Vec<[u8; 32]> = vec![
        agent_pubkey,     // 0: signer, writable
        provider_pubkey,  // 1: readonly
        usdc_mint,        // 2: readonly
        escrow_pda,       // 3: writable
        agent_ata,        // 4: writable
        vault_ata,        // 5: writable
        token_program,    // 6: readonly
        ata_program,      // 7: readonly
        system_program,   // 8: readonly
        // escrow_program_id appended separately as index 9
    ];

    // Step 7: Build instruction data
    // anchor_discriminator("deposit") + amount(u64 LE) + service_id([u8;32]) + expiry_slot(u64 LE)
    // = 8 + 8 + 32 + 8 = 56 bytes
    let discriminator = anchor_discriminator("deposit");
    let mut ix_data = Vec::with_capacity(56);
    ix_data.extend_from_slice(&discriminator);
    ix_data.extend_from_slice(&params.amount.to_le_bytes());
    ix_data.extend_from_slice(&params.service_id);
    ix_data.extend_from_slice(&params.expiry_slot.to_le_bytes());
    debug_assert_eq!(ix_data.len(), 56, "deposit ix_data must be 56 bytes");

    // Instruction account indices: all 9 data accounts + program at index 9
    let ix_account_indices: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8];

    // Step 8: Build the legacy message
    // Header: [1, 0, 6] — 1 signer, 0 readonly signed, 6 readonly unsigned
    // Readonly unsigned: provider(1), mint(2), token_program(6), ata_program(7),
    //                    system_program(8), escrow_program(9) = 6
    let msg = build_legacy_message(
        [1, 0, 6],
        &accounts,
        &escrow_program_id,
        &params.recent_blockhash,
        9u8, // program_id_index = 9 (last account)
        &ix_account_indices,
        &ix_data,
    );

    // Step 9: Sign the message with the agent keypair
    let signature = signing_key.sign(&msg);

    // Step 10: Assemble wire-format transaction
    // compact-u16(1) || signature(64) || message
    let mut tx_bytes = Vec::with_capacity(1 + 64 + msg.len());
    tx_bytes.push(0x01); // compact-u16: 1 signature
    tx_bytes.extend_from_slice(&signature.to_bytes());
    tx_bytes.extend_from_slice(&msg);

    // Step 11: Base64-encode and return
    Ok(base64::engine::general_purpose::STANDARD.encode(&tx_bytes))
}

// ---------------------------------------------------------------------------
// Helper: serialize Solana legacy message format
// ---------------------------------------------------------------------------

/// Serialize a Solana legacy transaction message.
///
/// Layout:
/// - header (3 bytes)
/// - compact-u16 account count + N×32 byte keys (data accounts + program key)
/// - 32-byte recent blockhash
/// - compact-u16 instruction count (always 1)
/// - instruction: program_id_index || compact-u16 accts || accts || compact-u16 data_len || data
fn build_legacy_message(
    header: [u8; 3],
    accounts: &[[u8; 32]],
    program_id: &[u8; 32],
    recent_blockhash: &[u8; 32],
    program_id_index: u8,
    ix_account_indices: &[u8],
    ix_data: &[u8],
) -> Vec<u8> {
    let total_accounts = accounts.len() + 1; // +1 for the program key
    debug_assert!(
        total_accounts <= 127,
        "compact-u16 single-byte encoding assumes <= 127 accounts; got {total_accounts}"
    );

    let mut msg = Vec::new();

    // Header
    msg.extend_from_slice(&header);

    // Account keys (compact-u16 count + keys)
    msg.push(total_accounts as u8);
    for acc in accounts {
        msg.extend_from_slice(acc);
    }
    msg.extend_from_slice(program_id); // program key is the last account

    // Recent blockhash
    msg.extend_from_slice(recent_blockhash);

    // Instruction count (compact-u16): always 1
    msg.push(1u8);

    // Instruction
    msg.push(program_id_index); // program_id_index
    msg.push(ix_account_indices.len() as u8); // compact-u16: account count
    msg.extend_from_slice(ix_account_indices); // account indices
    msg.push(ix_data.len() as u8); // compact-u16: data length
    msg.extend_from_slice(ix_data); // instruction data

    msg
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;

    const PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

    /// Build a deterministic 64-byte keypair from seed `[42u8; 32]`.
    fn test_agent_keypair_b58() -> String {
        let seed = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    fn base_params() -> DepositParams {
        DepositParams {
            agent_keypair_b58: test_agent_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 1_000_000,
            service_id: [1u8; 32],
            expiry_slot: 99_999_999,
            recent_blockhash: [0xABu8; 32],
        }
    }

    #[test]
    fn test_build_deposit_tx_produces_valid_base64() {
        let result = build_deposit_tx(&base_params());
        assert!(result.is_ok(), "build_deposit_tx failed: {:?}", result.err());
        let b64 = result.unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("output must be valid base64");
        assert!(
            decoded.len() > 100,
            "transaction too short: {} bytes",
            decoded.len()
        );
    }

    #[test]
    fn test_build_deposit_tx_contains_correct_discriminator() {
        let b64 = build_deposit_tx(&base_params()).expect("build should succeed");
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let disc = anchor_discriminator("deposit");
        let found = tx_bytes
            .windows(8)
            .any(|w| w == disc);
        assert!(found, "transaction bytes must contain the deposit discriminator");
    }

    #[test]
    fn test_build_deposit_tx_contains_agent_pubkey() {
        let keypair_b58 = test_agent_keypair_b58();
        let keypair_bytes = bs58::decode(&keypair_b58).into_vec().unwrap();
        let agent_pubkey = &keypair_bytes[32..64]; // stored pubkey portion

        let b64 = build_deposit_tx(&base_params()).expect("build should succeed");
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();

        let found = tx_bytes.windows(32).any(|w| w == agent_pubkey);
        assert!(found, "transaction bytes must contain the agent pubkey");
    }

    #[test]
    fn test_build_deposit_tx_zero_amount_rejected() {
        let mut params = base_params();
        params.amount = 0;
        let result = build_deposit_tx(&params);
        assert!(result.is_err(), "zero amount should be rejected");
        let err = result.unwrap_err();
        assert!(
            err.to_lowercase().contains("zero") || err.contains("non-zero"),
            "error message should mention zero: {err}"
        );
    }

    #[test]
    fn test_build_deposit_tx_invalid_keypair_rejected() {
        let mut params = base_params();
        params.agent_keypair_b58 = "notavalidkeypair!!!".to_string();
        let result = build_deposit_tx(&params);
        assert!(result.is_err(), "invalid keypair should be rejected");
    }
}
