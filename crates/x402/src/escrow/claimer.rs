//! EscrowClaimer — fire-and-forget claim submission.

use tracing::{info, warn};

use crate::traits::Error;

use super::pda::{
    anchor_discriminator, decode_bs58_pubkey, derive_ata_address, find_program_address,
    ATA_PROGRAM_ID, SYSTEM_PROGRAM_ID, SYSVAR_RENT_ID, TOKEN_PROGRAM_ID,
};

// ---------------------------------------------------------------------------
// EscrowClaimer
// ---------------------------------------------------------------------------

/// Submits claim transactions to the escrow program after successful LLM responses.
///
/// The claim transfers the actual cost from the escrow vault to the provider
/// and refunds the remainder to the agent.
///
/// `Debug` intentionally omits the `fee_payer_keypair` field to prevent
/// secret key material from appearing in log output.
pub struct EscrowClaimer {
    rpc_url: String,
    fee_payer_keypair: [u8; 64],
    escrow_program_id: [u8; 32],
    recipient_wallet: [u8; 32],
    usdc_mint: [u8; 32],
    /// Shared HTTP client for RPC calls (avoids per-call allocation).
    client: reqwest::Client,
}

impl std::fmt::Debug for EscrowClaimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EscrowClaimer")
            .field("rpc_url", &self.rpc_url)
            .field("fee_payer_keypair", &"[REDACTED]")
            .field(
                "escrow_program_id",
                &bs58::encode(&self.escrow_program_id).into_string(),
            )
            .field(
                "recipient_wallet",
                &bs58::encode(&self.recipient_wallet).into_string(),
            )
            .field("usdc_mint", &bs58::encode(&self.usdc_mint).into_string())
            .finish()
    }
}

impl Drop for EscrowClaimer {
    fn drop(&mut self) {
        // Zero out secret key material on drop.
        self.fee_payer_keypair.iter_mut().for_each(|b| *b = 0);
    }
}

impl EscrowClaimer {
    /// Create a new EscrowClaimer.
    ///
    /// # Arguments
    /// * `rpc_url` — Solana RPC endpoint
    /// * `fee_payer_b58_key` — Base58-encoded ed25519 secret key (64 bytes)
    /// * `escrow_program_id_b58` — Base58-encoded escrow program ID
    /// * `recipient_wallet_b58` — Base58-encoded provider wallet pubkey
    /// * `usdc_mint_b58` — Base58-encoded USDC mint pubkey
    pub fn new(
        rpc_url: String,
        fee_payer_b58_key: &str,
        escrow_program_id_b58: &str,
        recipient_wallet_b58: &str,
        usdc_mint_b58: &str,
    ) -> Result<Self, String> {
        let key_bytes = bs58::decode(fee_payer_b58_key)
            .into_vec()
            .map_err(|e| format!("invalid fee_payer_key base58: {e}"))?;
        if key_bytes.len() != 64 {
            return Err(format!(
                "fee_payer_key must be 64 bytes, got {}",
                key_bytes.len()
            ));
        }
        let mut fee_payer_keypair = [0u8; 64];
        fee_payer_keypair.copy_from_slice(&key_bytes);

        let escrow_program_id = decode_bs58_pubkey(escrow_program_id_b58)
            .map_err(|e| format!("escrow_program_id: {e}"))?;
        let recipient_wallet = decode_bs58_pubkey(recipient_wallet_b58)
            .map_err(|e| format!("recipient_wallet: {e}"))?;
        let usdc_mint = decode_bs58_pubkey(usdc_mint_b58).map_err(|e| format!("usdc_mint: {e}"))?;

        Ok(Self {
            rpc_url,
            fee_payer_keypair,
            escrow_program_id,
            recipient_wallet,
            usdc_mint,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        })
    }

    /// Fire-and-forget claim submission. Does not block the caller.
    pub fn claim_async(
        &self,
        service_id: [u8; 32],
        agent_pubkey: [u8; 32],
        actual_amount_atomic: u64,
    ) {
        let params = ClaimParams {
            rpc_url: self.rpc_url.clone(),
            fee_payer_keypair: self.fee_payer_keypair,
            escrow_program_id: self.escrow_program_id,
            recipient_wallet: self.recipient_wallet,
            usdc_mint: self.usdc_mint,
            service_id,
            agent_pubkey,
            actual_amount: actual_amount_atomic,
            client: self.client.clone(),
        };

        tokio::spawn(async move {
            if let Err(e) = do_claim(&params).await {
                warn!(
                    error = %e,
                    agent = %bs58::encode(&params.agent_pubkey).into_string(),
                    amount = params.actual_amount,
                    "escrow claim failed"
                );
            }
        });
    }
}

/// Parameters for a claim transaction, bundled to avoid too-many-arguments.
pub(crate) struct ClaimParams {
    rpc_url: String,
    fee_payer_keypair: [u8; 64],
    escrow_program_id: [u8; 32],
    recipient_wallet: [u8; 32],
    usdc_mint: [u8; 32],
    service_id: [u8; 32],
    pub(crate) agent_pubkey: [u8; 32],
    pub(crate) actual_amount: u64,
    /// Shared HTTP client (avoids per-call allocation).
    client: reqwest::Client,
}

/// Internal: build and submit the claim transaction.
pub(crate) async fn do_claim(params: &ClaimParams) -> Result<(), Error> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    let http_client = &params.client;

    // Step 1: Get latest blockhash
    let bh_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": [{"commitment": "confirmed"}],
    });

    let bh_response: serde_json::Value = http_client
        .post(&params.rpc_url)
        .json(&bh_body)
        .send()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?
        .json()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    let blockhash_str = bh_response
        .get("result")
        .and_then(|r| r.get("value"))
        .and_then(|v| v.get("blockhash"))
        .and_then(|b| b.as_str())
        .ok_or_else(|| Error::Rpc("failed to get latest blockhash".to_string()))?;

    let blockhash_bytes = bs58::decode(blockhash_str)
        .into_vec()
        .map_err(|e| Error::Rpc(format!("invalid blockhash: {e}")))?;
    if blockhash_bytes.len() != 32 {
        return Err(Error::Rpc("blockhash must be 32 bytes".to_string()));
    }
    let mut blockhash = [0u8; 32];
    blockhash.copy_from_slice(&blockhash_bytes);

    // Step 2: Derive all addresses
    let (escrow_pda, _escrow_bump) = find_program_address(
        &[b"escrow", &params.agent_pubkey, &params.service_id],
        &params.escrow_program_id,
    )
    .ok_or_else(|| Error::EscrowClaimFailed("failed to derive escrow PDA".to_string()))?;

    let vault_ata = derive_ata_address(&escrow_pda, &params.usdc_mint)
        .ok_or_else(|| Error::EscrowClaimFailed("failed to derive vault ATA".to_string()))?;

    let provider_ata = derive_ata_address(&params.recipient_wallet, &params.usdc_mint)
        .ok_or_else(|| Error::EscrowClaimFailed("failed to derive provider ATA".to_string()))?;

    let agent_ata = derive_ata_address(&params.agent_pubkey, &params.usdc_mint)
        .ok_or_else(|| Error::EscrowClaimFailed("failed to derive agent ATA".to_string()))?;

    let token_program = decode_bs58_pubkey(TOKEN_PROGRAM_ID)
        .map_err(|e| Error::EscrowClaimFailed(format!("token program: {e}")))?;
    let ata_program = decode_bs58_pubkey(ATA_PROGRAM_ID)
        .map_err(|e| Error::EscrowClaimFailed(format!("ata program: {e}")))?;
    let system_program = decode_bs58_pubkey(SYSTEM_PROGRAM_ID)
        .map_err(|e| Error::EscrowClaimFailed(format!("system program: {e}")))?;
    let _sysvar_rent = decode_bs58_pubkey(SYSVAR_RENT_ID)
        .map_err(|e| Error::EscrowClaimFailed(format!("sysvar rent: {e}")))?;

    // Step 3: Build Anchor claim instruction data
    // 8-byte discriminator + u64 actual_amount (little-endian)
    let discriminator = anchor_discriminator("claim");
    let mut ix_data = Vec::with_capacity(16);
    ix_data.extend_from_slice(&discriminator);
    ix_data.extend_from_slice(&params.actual_amount.to_le_bytes());

    // Step 4: Build the fee payer pubkey from the keypair
    let signing_key = SigningKey::from_keypair_bytes(&params.fee_payer_keypair)
        .map_err(|e| Error::EscrowClaimFailed(format!("invalid fee_payer keypair: {e}")))?;
    let fee_payer_pubkey = signing_key.verifying_key().to_bytes();

    // Step 5: Build the legacy message
    // Account ordering follows the Claim accounts struct in claim.rs:
    //   escrow (mut), agent (mut), provider (signer, mut), mint,
    //   vault (mut), provider_token_account (mut), agent_token_account (mut),
    //   token_program, associated_token_program, system_program
    //
    // FIX 6: Only the single-signer path (fee_payer == provider) is supported.
    // The two-signer path would require the provider's separate private key
    // which the gateway does not have. Reject early with a clear error.
    if fee_payer_pubkey != params.recipient_wallet {
        return Err(Error::EscrowClaimFailed(
            "fee_payer_key must belong to the recipient_wallet; \
             separate fee payer and provider keys are not supported \
             because only the fee_payer_key is available for signing"
                .to_string(),
        ));
    }

    let accounts: Vec<[u8; 32]> = vec![
        fee_payer_pubkey,    // 0: provider/fee_payer (signer, writable)
        escrow_pda,          // 1: escrow PDA (writable)
        params.agent_pubkey, // 2: agent (writable)
        params.usdc_mint,    // 3: mint
        vault_ata,           // 4: vault (writable)
        provider_ata,        // 5: provider_token_account (writable)
        agent_ata,           // 6: agent_token_account (writable)
        token_program,       // 7: token_program
        ata_program,         // 8: associated_token_program
        system_program,      // 9: system_program
    ];

    // 1 signer (fee_payer=provider), 0 readonly signed, 3 readonly unsigned (mint, token, ata, system)
    let (num_required_signatures, num_readonly_signed, num_readonly_unsigned, program_id_index) =
        (1u8, 0u8, 3u8, 9u8);
    let ix_accounts: Vec<u8> = vec![1u8, 2, 0, 3, 4, 5, 6, 7, 8, 9];

    // Build message bytes
    let mut msg = Vec::new();
    msg.push(num_required_signatures);
    msg.push(num_readonly_signed);
    msg.push(num_readonly_unsigned);

    // Account keys count (compact-u16)
    // For simplicity, accounts.len() + 1 (escrow program) — embed program as last account
    let total_accounts = accounts.len() + 1; // +1 for the escrow program itself
    debug_assert!(
        total_accounts <= 127,
        "compact-u16 encoding assumes single byte; got {total_accounts}"
    );
    msg.push(total_accounts as u8);

    // Account keys
    for acc in &accounts {
        msg.extend_from_slice(acc);
    }
    msg.extend_from_slice(&params.escrow_program_id); // program ID as last account

    // Recent blockhash
    msg.extend_from_slice(&blockhash);

    // Instructions: 1 instruction
    msg.push(1u8); // compact-u16: 1 instruction

    // The instruction
    msg.push(program_id_index + 1); // +1 because we added program_id at the end
    msg.push(ix_accounts.len() as u8); // compact-u16: account count
    msg.extend_from_slice(&ix_accounts);
    msg.push(ix_data.len() as u8); // compact-u16: data length
    msg.extend_from_slice(&ix_data);

    // Step 6: Sign the message
    let signature = signing_key.sign(&msg);

    // Step 7: Build the full transaction (single signer)
    let mut tx_bytes = Vec::new();
    tx_bytes.push(0x01); // compact-u16: 1 signature
    tx_bytes.extend_from_slice(&signature.to_bytes());
    tx_bytes.extend_from_slice(&msg);

    // Step 8: Base64-encode and submit
    let tx_base64 = base64::engine::general_purpose::STANDARD.encode(&tx_bytes);

    let send_body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "sendTransaction",
        "params": [tx_base64, {
            "encoding": "base64",
            "skipPreflight": false,
            "preflightCommitment": "confirmed"
        }],
    });

    let send_response: serde_json::Value = http_client
        .post(&params.rpc_url)
        .json(&send_body)
        .send()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?
        .json()
        .await
        .map_err(|e| Error::Rpc(e.to_string()))?;

    if let Some(error) = send_response.get("error") {
        return Err(Error::EscrowClaimFailed(format!(
            "sendTransaction error: {error}"
        )));
    }

    let tx_sig = send_response
        .get("result")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            Error::EscrowClaimFailed("sendTransaction did not return a signature".to_string())
        })?;

    info!(
        signature = %tx_sig,
        amount = params.actual_amount,
        "escrow claim transaction submitted"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escrow_claimer_new_invalid_key() {
        let result = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            "not-valid-base58!!!",
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_escrow_claimer_new_wrong_length_key() {
        // Valid base58 but only 32 bytes instead of 64
        let result = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            "11111111111111111111111111111111", // 32 bytes of zeros
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        );
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("64 bytes"));
    }
}
