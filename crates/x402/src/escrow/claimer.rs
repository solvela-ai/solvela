//! EscrowClaimer — fire-and-forget claim submission.
//!
//! Supports fee payer rotation via `FeePayerPool` (Phase 8.2) and optional
//! durable nonces via `NoncePool` (Phase 8.3).

use std::sync::Arc;

use tracing::{info, warn};

use crate::fee_payer::FeePayerPool;
use crate::nonce_pool::NoncePool;
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
/// Uses `FeePayerPool` for round-robin fee payer rotation with health tracking.
/// When a claim fails with an RPC error suggesting insufficient SOL, the fee
/// payer wallet is marked unhealthy and the next claim will use a different one.
///
/// Optionally uses `NoncePool` for durable nonces in claim transactions.
/// When a nonce is available, it replaces the recent blockhash to avoid
/// the ~60s expiry window.
///
/// `Debug` intentionally omits secret key material.
pub struct EscrowClaimer {
    rpc_url: String,
    fee_payer_pool: Arc<FeePayerPool>,
    escrow_program_id: [u8; 32],
    recipient_wallet: [u8; 32],
    usdc_mint: [u8; 32],
    /// Optional nonce pool for durable nonce transactions (Phase 8.3).
    nonce_pool: Option<Arc<NoncePool>>,
    /// Shared HTTP client for RPC calls (avoids per-call allocation).
    client: reqwest::Client,
}

impl std::fmt::Debug for EscrowClaimer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EscrowClaimer")
            .field("rpc_url", &self.rpc_url)
            .field("fee_payer_pool", &self.fee_payer_pool)
            .field(
                "escrow_program_id",
                &bs58::encode(&self.escrow_program_id).into_string(),
            )
            .field(
                "recipient_wallet",
                &bs58::encode(&self.recipient_wallet).into_string(),
            )
            .field("usdc_mint", &bs58::encode(&self.usdc_mint).into_string())
            .field("nonce_pool", &self.nonce_pool.as_ref().map(|p| p.len()))
            .finish()
    }
}

impl EscrowClaimer {
    /// Create a new EscrowClaimer with fee payer pool rotation.
    ///
    /// # Arguments
    /// * `rpc_url` — Solana RPC endpoint
    /// * `fee_payer_pool` — Pool of fee payer wallets for rotation
    /// * `escrow_program_id_b58` — Base58-encoded escrow program ID
    /// * `recipient_wallet_b58` — Base58-encoded provider wallet pubkey
    /// * `usdc_mint_b58` — Base58-encoded USDC mint pubkey
    /// * `nonce_pool` — Optional durable nonce pool for claim transactions
    pub fn new(
        rpc_url: String,
        fee_payer_pool: Arc<FeePayerPool>,
        escrow_program_id_b58: &str,
        recipient_wallet_b58: &str,
        usdc_mint_b58: &str,
        nonce_pool: Option<Arc<NoncePool>>,
    ) -> Result<Self, String> {
        let escrow_program_id = decode_bs58_pubkey(escrow_program_id_b58)
            .map_err(|e| format!("escrow_program_id: {e}"))?;
        let recipient_wallet = decode_bs58_pubkey(recipient_wallet_b58)
            .map_err(|e| format!("recipient_wallet: {e}"))?;
        let usdc_mint = decode_bs58_pubkey(usdc_mint_b58).map_err(|e| format!("usdc_mint: {e}"))?;

        Ok(Self {
            rpc_url,
            fee_payer_pool,
            escrow_program_id,
            recipient_wallet,
            usdc_mint,
            nonce_pool,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| format!("failed to build HTTP client: {e}"))?,
        })
    }

    /// Fire-and-forget claim submission. Does not block the caller.
    pub fn claim_async(
        &self,
        service_id: [u8; 32],
        agent_pubkey: [u8; 32],
        actual_amount_atomic: u64,
    ) {
        // Select fee payer from the pool
        let wallet = match self.fee_payer_pool.next() {
            Ok(w) => w,
            Err(e) => {
                warn!(
                    error = %e,
                    agent = %bs58::encode(&agent_pubkey).into_string(),
                    "no healthy fee payer available for claim — skipping"
                );
                return;
            }
        };

        let params = ClaimParams {
            rpc_url: self.rpc_url.clone(),
            fee_payer_keypair: *wallet.keypair_bytes(),
            fee_payer_index: wallet.index,
            escrow_program_id: self.escrow_program_id,
            recipient_wallet: self.recipient_wallet,
            usdc_mint: self.usdc_mint,
            service_id,
            agent_pubkey,
            actual_amount: actual_amount_atomic,
            client: self.client.clone(),
            nonce_pool: self.nonce_pool.clone(),
            nonce_rpc_url: self.rpc_url.clone(),
        };
        let pool = Arc::clone(&self.fee_payer_pool);

        tokio::spawn(async move {
            match do_claim(&params).await {
                Ok(sig) => {
                    info!(
                        signature = %sig,
                        agent = %bs58::encode(&params.agent_pubkey).into_string(),
                        amount = params.actual_amount,
                        "escrow claim succeeded (fire-and-forget)"
                    );
                }
                Err(e) => {
                    // Mark fee payer unhealthy if error suggests insufficient SOL
                    if is_insufficient_sol_error(&e) {
                        pool.mark_failed(params.fee_payer_index);
                        warn!(
                            error = %e,
                            fee_payer_index = params.fee_payer_index,
                            "fee payer marked unhealthy — insufficient SOL"
                        );
                    }
                    warn!(
                        error = %e,
                        agent = %bs58::encode(&params.agent_pubkey).into_string(),
                        amount = params.actual_amount,
                        "escrow claim failed"
                    );
                }
            }
        });
    }
}

/// Parameters for a claim transaction, bundled to avoid too-many-arguments.
pub(crate) struct ClaimParams {
    rpc_url: String,
    fee_payer_keypair: [u8; 64],
    fee_payer_index: usize,
    escrow_program_id: [u8; 32],
    recipient_wallet: [u8; 32],
    usdc_mint: [u8; 32],
    service_id: [u8; 32],
    pub(crate) agent_pubkey: [u8; 32],
    pub(crate) actual_amount: u64,
    /// Shared HTTP client (avoids per-call allocation).
    client: reqwest::Client,
    /// Optional nonce pool for durable nonce transactions.
    nonce_pool: Option<Arc<NoncePool>>,
    /// RPC URL for nonce value fetching (same as main RPC).
    nonce_rpc_url: String,
}

impl Drop for ClaimParams {
    fn drop(&mut self) {
        self.fee_payer_keypair.iter_mut().for_each(|b| *b = 0);
    }
}

/// Check if an error indicates the fee payer has insufficient SOL.
fn is_insufficient_sol_error(error: &Error) -> bool {
    let msg = error.to_string().to_lowercase();
    msg.contains("insufficient lamports")
        || msg.contains("insufficient funds")
        || msg.contains("insufficient sol")
        || msg.contains("0x1") // InsufficientFunds error code
}

/// Submit a claim and return the transaction signature.
/// Unlike `claim_async`, this awaits the result.
pub async fn do_claim_with_params(
    claimer: &EscrowClaimer,
    service_id: [u8; 32],
    agent_pubkey: [u8; 32],
    actual_amount_atomic: u64,
) -> Result<String, Error> {
    // Select fee payer from the pool
    let wallet = claimer
        .fee_payer_pool
        .next()
        .map_err(|e| Error::EscrowClaimFailed(format!("no healthy fee payer: {e}")))?;

    let params = ClaimParams {
        rpc_url: claimer.rpc_url.clone(),
        fee_payer_keypair: *wallet.keypair_bytes(),
        fee_payer_index: wallet.index,
        escrow_program_id: claimer.escrow_program_id,
        recipient_wallet: claimer.recipient_wallet,
        usdc_mint: claimer.usdc_mint,
        service_id,
        agent_pubkey,
        actual_amount: actual_amount_atomic,
        client: claimer.client.clone(),
        nonce_pool: claimer.nonce_pool.clone(),
        nonce_rpc_url: claimer.rpc_url.clone(),
    };

    let result = do_claim(&params).await;

    // Mark fee payer unhealthy on insufficient SOL errors
    if let Err(ref e) = result {
        if is_insufficient_sol_error(e) {
            claimer.fee_payer_pool.mark_failed(wallet.index);
            warn!(
                error = %e,
                fee_payer_index = wallet.index,
                "fee payer marked unhealthy — insufficient SOL"
            );
        }
    }

    result
}

/// Internal: build and submit the claim transaction.
pub(crate) async fn do_claim(params: &ClaimParams) -> Result<String, Error> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    let http_client = &params.client;

    // Step 1: Get blockhash — prefer durable nonce, fall back to recent blockhash.
    // When using a durable nonce, an AdvanceNonceAccount instruction must be
    // prepended to the transaction.
    let (blockhash, nonce_advance_ix) = match try_fetch_nonce(params).await {
        Some((nonce_hash, advance_ix)) => {
            info!("using durable nonce for claim transaction");
            (nonce_hash, Some(advance_ix))
        }
        None => {
            // Fall back to regular blockhash
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
            let mut bh = [0u8; 32];
            bh.copy_from_slice(&blockhash_bytes);
            (bh, None)
        }
    };

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

    // Build the account list — if using a nonce, we need nonce-related accounts too.
    let mut accounts: Vec<[u8; 32]> = vec![
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

    // If using durable nonce, append nonce account and SysvarRecentBlockhashes
    // to the account list and build the AdvanceNonce instruction.
    let nonce_advance_accounts = if let Some(ref advance_ix) = nonce_advance_ix {
        // Nonce account (writable) and SysvarRecentBlockhashes (readonly)
        accounts.push(advance_ix.nonce_account); // 10: nonce account (writable)
        accounts.push(advance_ix.sysvar_recent_blockhashes); // 11: sysvar recent blockhashes
        Some((accounts.len() - 2, accounts.len() - 1)) // indices of nonce account and sysvar
    } else {
        None
    };

    // 1 signer (fee_payer=provider), 0 readonly signed, varying readonly unsigned
    // Base readonly unsigned: mint(3), token_program(7), ata_program(8), system_program(9) = 3
    // With nonce: add sysvar_recent_blockhashes = 4
    let num_readonly_unsigned = if nonce_advance_accounts.is_some() {
        4u8
    } else {
        3u8
    };
    let (num_required_signatures, num_readonly_signed) = (1u8, 0u8);

    let ix_accounts: Vec<u8> = vec![1u8, 2, 0, 3, 4, 5, 6, 7, 8, 9];

    // Build message bytes
    let mut msg = Vec::new();
    msg.push(num_required_signatures);
    msg.push(num_readonly_signed);
    msg.push(num_readonly_unsigned);

    // Account keys count (compact-u16)
    // accounts.len() + 1 (escrow program) — embed program as last account
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

    // Recent blockhash (or durable nonce value)
    msg.extend_from_slice(&blockhash);

    // Instructions count
    let num_instructions: u8 = if nonce_advance_accounts.is_some() {
        2 // AdvanceNonce + Claim
    } else {
        1 // Claim only
    };
    msg.push(num_instructions);

    // If using nonce, prepend the AdvanceNonceAccount instruction
    if let Some((nonce_acct_idx, sysvar_idx)) = nonce_advance_accounts {
        // Program ID for AdvanceNonce is SystemProgram (index 9 in accounts)
        msg.push(9u8); // system_program index
        msg.push(3u8); // 3 accounts: nonce_account, sysvar_recent_blockhashes, nonce_authority(=fee_payer)
        msg.push(nonce_acct_idx as u8); // nonce account (writable)
        msg.push(sysvar_idx as u8); // sysvar_recent_blockhashes
        msg.push(0u8); // nonce authority = fee_payer (index 0, signer)
        msg.push(4u8); // data length: 4 bytes
        msg.extend_from_slice(&4u32.to_le_bytes()); // AdvanceNonceAccount instruction index
    }

    // The claim instruction
    let escrow_program_idx = (accounts.len()) as u8; // escrow program is the last account added
    msg.push(escrow_program_idx);
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

    Ok(tx_sig.to_string())
}

// ---------------------------------------------------------------------------
// Durable nonce helper
// ---------------------------------------------------------------------------

/// SysvarRecentBlockhashes pubkey (used in AdvanceNonceAccount instruction).
const SYSVAR_RECENT_BLOCKHASHES_ID: &str = "SysvarRecentB1teleworLdhashes11111111111111";

/// Data needed to build an AdvanceNonceAccount instruction.
struct NonceAdvanceInfo {
    nonce_account: [u8; 32],
    sysvar_recent_blockhashes: [u8; 32],
}

/// Try to fetch a durable nonce from the nonce pool.
///
/// Returns `Some((nonce_hash_bytes, advance_info))` on success,
/// `None` if the nonce pool is unavailable, empty, or the RPC call fails
/// (with a warning logged).
async fn try_fetch_nonce(params: &ClaimParams) -> Option<([u8; 32], NonceAdvanceInfo)> {
    let nonce_pool = params.nonce_pool.as_ref()?;

    let entry = nonce_pool.next()?;

    match nonce_pool
        .fetch_nonce_value(&params.nonce_rpc_url, entry)
        .await
    {
        Ok(nonce_value_b58) => {
            let nonce_bytes = match bs58::decode(&nonce_value_b58).into_vec() {
                Ok(bytes) if bytes.len() == 32 => {
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    arr
                }
                _ => {
                    warn!(
                        nonce_account = %entry.nonce_account,
                        "durable nonce value has invalid length — falling back to recent blockhash"
                    );
                    return None;
                }
            };

            let nonce_account = match decode_bs58_pubkey(&entry.nonce_account) {
                Ok(pk) => pk,
                Err(e) => {
                    warn!(error = %e, "invalid nonce account pubkey — falling back to recent blockhash");
                    return None;
                }
            };

            let sysvar_recent_blockhashes = match decode_bs58_pubkey(SYSVAR_RECENT_BLOCKHASHES_ID) {
                Ok(pk) => pk,
                Err(e) => {
                    warn!(error = %e, "failed to decode sysvar pubkey — falling back to recent blockhash");
                    return None;
                }
            };

            Some((
                nonce_bytes,
                NonceAdvanceInfo {
                    nonce_account,
                    sysvar_recent_blockhashes,
                },
            ))
        }
        Err(e) => {
            warn!(
                error = %e,
                nonce_account = %entry.nonce_account,
                "failed to fetch durable nonce value — falling back to recent blockhash"
            );
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fee_payer::FeePayerPool;
    use crate::nonce_pool::NoncePool;

    /// Generate a deterministic valid base58-encoded ed25519 keypair (64 bytes)
    fn test_keypair_b58(seed: u8) -> String {
        use ed25519_dalek::SigningKey;
        let mut secret = [0u8; 32];
        secret[0] = seed;
        secret[31] = seed.wrapping_add(1);
        let signing_key = SigningKey::from_bytes(&secret);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    fn make_pool(keys: &[String]) -> Arc<FeePayerPool> {
        Arc::new(FeePayerPool::from_keys(keys).expect("pool should load"))
    }

    #[test]
    fn test_escrow_claimer_new_invalid_program_id() {
        let keys = vec![test_keypair_b58(1)];
        let pool = make_pool(&keys);
        let result = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            pool,
            "not-valid-base58!!!",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            None,
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_escrow_claimer_new_valid_construction() {
        let keys = vec![test_keypair_b58(1)];
        let pool = make_pool(&keys);
        let result = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            pool,
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            None,
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_escrow_claimer_new_with_nonce_pool() {
        let keys = vec![test_keypair_b58(1)];
        let pool = make_pool(&keys);
        let nonce_pool =
            Arc::new(NoncePool::from_entries(vec![]).expect("empty nonce pool should be Ok"));
        let result = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            pool,
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            Some(nonce_pool),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn test_escrow_claimer_debug_does_not_leak_keys() {
        let keys = vec![test_keypair_b58(1)];
        let pool = make_pool(&keys);
        let claimer = EscrowClaimer::new(
            "https://api.devnet.solana.com".to_string(),
            pool,
            "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy",
            "11111111111111111111111111111111",
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            None,
        )
        .expect("should construct");
        let debug = format!("{claimer:?}");
        // Should not contain raw keypair bytes
        assert!(!debug.contains(&test_keypair_b58(1)));
    }

    #[test]
    fn test_is_insufficient_sol_error() {
        assert!(is_insufficient_sol_error(&Error::Rpc(
            "Transaction simulation failed: Transaction results in an account (0) with insufficient lamports".to_string()
        )));
        assert!(is_insufficient_sol_error(&Error::Rpc(
            "insufficient funds for rent".to_string()
        )));
        assert!(!is_insufficient_sol_error(&Error::Rpc(
            "some other error".to_string()
        )));
    }

    #[test]
    fn test_nonce_fallback_when_pool_empty() {
        // When nonce_pool has no entries, try_fetch_nonce returns None
        let empty_pool = Arc::new(NoncePool::from_entries(vec![]).expect("empty pool ok"));
        let params = ClaimParams {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            fee_payer_keypair: [0u8; 64],
            fee_payer_index: 0,
            escrow_program_id: [0u8; 32],
            recipient_wallet: [0u8; 32],
            usdc_mint: [0u8; 32],
            service_id: [0u8; 32],
            agent_pubkey: [0u8; 32],
            actual_amount: 0,
            client: reqwest::Client::new(),
            nonce_pool: Some(empty_pool),
            nonce_rpc_url: "https://api.devnet.solana.com".to_string(),
        };
        // try_fetch_nonce is async — use a block_on for sync test
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(try_fetch_nonce(&params));
        assert!(result.is_none(), "empty nonce pool should return None");
    }

    #[test]
    fn test_nonce_fallback_when_pool_none() {
        let params = ClaimParams {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            fee_payer_keypair: [0u8; 64],
            fee_payer_index: 0,
            escrow_program_id: [0u8; 32],
            recipient_wallet: [0u8; 32],
            usdc_mint: [0u8; 32],
            service_id: [0u8; 32],
            agent_pubkey: [0u8; 32],
            actual_amount: 0,
            client: reqwest::Client::new(),
            nonce_pool: None,
            nonce_rpc_url: "https://api.devnet.solana.com".to_string(),
        };
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(try_fetch_nonce(&params));
        assert!(result.is_none(), "None nonce pool should return None");
    }
}
