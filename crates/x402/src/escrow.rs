//! EscrowVerifier — verifies on-chain escrow deposits and fires claim transactions.
//!
//! The EscrowVerifier handles scheme="escrow" payments where agents deposit
//! to a PDA vault rather than sending a direct SPL transfer. After the gateway
//! proxies the request to the LLM provider, the EscrowClaimer fires a
//! fire-and-forget claim transaction to collect the actual cost from the vault.

use async_trait::async_trait;
use tracing::{info, warn};

use crate::solana_types::{Pubkey, VersionedTransaction};
use crate::traits::{Error, PaymentVerifier};
use crate::types::{
    PayloadData, PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const ATA_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJe1bxs";
const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
const SYSVAR_RENT_ID: &str = "SysvarRent111111111111111111111111111111111";

// ---------------------------------------------------------------------------
// PDA derivation helpers
// ---------------------------------------------------------------------------

/// Derive a Program Derived Address using SHA-256 (same as Solana runtime).
///
/// Returns `(pubkey_bytes, bump)` or `None` if no valid off-curve point is found.
fn find_program_address(seeds: &[&[u8]], program_id: &[u8; 32]) -> Option<([u8; 32], u8)> {
    use sha2::{Digest, Sha256};

    for nonce in (0u8..=255).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([nonce]);
        hasher.update(b"ProgramDerivedAddress");
        hasher.update(program_id);
        let hash = hasher.finalize();

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);

        if !is_on_ed25519_curve(&bytes) {
            return Some((bytes, nonce));
        }
    }

    None
}

/// Check if 32 bytes represent a valid compressed point on the ed25519 curve.
fn is_on_ed25519_curve(bytes: &[u8; 32]) -> bool {
    use curve25519_dalek::edwards::CompressedEdwardsY;
    CompressedEdwardsY(*bytes).decompress().is_some()
}

/// Derive the Associated Token Account address for a given wallet and mint.
fn derive_ata_address(wallet: &[u8; 32], mint: &[u8; 32]) -> Option<[u8; 32]> {
    let token_program = decode_bs58_pubkey(TOKEN_PROGRAM_ID).ok()?;
    let ata_program = decode_bs58_pubkey(ATA_PROGRAM_ID).ok()?;

    let seeds: &[&[u8]] = &[wallet, &token_program, mint];
    find_program_address(seeds, &ata_program).map(|(addr, _)| addr)
}

/// Decode a base58-encoded pubkey into 32 bytes.
fn decode_bs58_pubkey(s: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| format!("invalid base58: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Compute the Anchor instruction discriminator: sha256("global:<name>")[..8].
fn anchor_discriminator(name: &str) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{name}"));
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

// ---------------------------------------------------------------------------
// SPL Transfer extraction (mirrors solana.rs logic for deposit tx inspection)
// ---------------------------------------------------------------------------

/// Information extracted from an SPL Token transfer instruction.
struct SplTransferInfo {
    /// The destination token account.
    destination: Pubkey,
    /// Transfer amount in atomic units.
    amount: u64,
    /// Mint address (only present for TransferChecked).
    mint: Option<Pubkey>,
}

/// Extract SPL Token transfer information from a parsed message.
///
/// Searches for SPL Token `Transfer` (discriminator 3) or `TransferChecked`
/// (discriminator 12) instructions. Returns the first matching transfer.
fn extract_spl_transfer(
    message: &crate::solana_types::ParsedMessage,
) -> Result<SplTransferInfo, Error> {
    for ix in &message.instructions {
        let program_id_index = ix.program_id_index as usize;
        if program_id_index >= message.account_keys.len() {
            continue;
        }

        let program_id = &message.account_keys[program_id_index];

        // Check if this is an SPL Token program instruction
        let is_token_program =
            *program_id == Pubkey::TOKEN_PROGRAM_ID || *program_id == Pubkey::TOKEN_2022_PROGRAM_ID;
        if !is_token_program {
            continue;
        }

        if ix.data.is_empty() {
            continue;
        }

        match ix.data[0] {
            // Transfer: discriminator=3, data[1..9]=amount(u64 LE)
            // accounts: [source, destination, authority]
            3 => {
                if ix.data.len() < 9 {
                    return Err(Error::InvalidTransaction(
                        "SPL Transfer instruction data too short".to_string(),
                    ));
                }
                if ix.accounts.len() < 2 {
                    return Err(Error::InvalidTransaction(
                        "SPL Transfer instruction missing accounts".to_string(),
                    ));
                }

                let amount = u64::from_le_bytes(ix.data[1..9].try_into().map_err(|_| {
                    Error::InvalidTransaction("failed to parse transfer amount".to_string())
                })?);

                let dest_index = ix.accounts[1] as usize;
                if dest_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "destination account index out of bounds".to_string(),
                    ));
                }
                let destination = message.account_keys[dest_index];

                return Ok(SplTransferInfo {
                    destination,
                    amount,
                    mint: None,
                });
            }
            // TransferChecked: discriminator=12, data[1..9]=amount(u64 LE)
            // accounts: [source, mint, destination, authority]
            12 => {
                if ix.data.len() < 9 {
                    return Err(Error::InvalidTransaction(
                        "SPL TransferChecked instruction data too short".to_string(),
                    ));
                }
                if ix.accounts.len() < 3 {
                    return Err(Error::InvalidTransaction(
                        "SPL TransferChecked instruction missing accounts".to_string(),
                    ));
                }

                let amount = u64::from_le_bytes(ix.data[1..9].try_into().map_err(|_| {
                    Error::InvalidTransaction("failed to parse transfer amount".to_string())
                })?);

                let mint_index = ix.accounts[1] as usize;
                if mint_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "mint account index out of bounds".to_string(),
                    ));
                }
                let mint = message.account_keys[mint_index];

                let dest_index = ix.accounts[2] as usize;
                if dest_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "destination account index out of bounds".to_string(),
                    ));
                }
                let destination = message.account_keys[dest_index];

                return Ok(SplTransferInfo {
                    destination,
                    amount,
                    mint: Some(mint),
                });
            }
            _ => continue,
        }
    }

    Err(Error::InvalidTransaction(
        "no SPL Token transfer instruction found in deposit tx".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// EscrowVerifier
// ---------------------------------------------------------------------------

/// Verifies escrow deposit transactions for scheme="escrow" payments.
pub struct EscrowVerifier {
    /// Solana RPC endpoint URL.
    pub rpc_url: String,
    /// The gateway's wallet = escrow provider (base58).
    pub recipient_wallet: String,
    /// USDC mint address (base58).
    pub usdc_mint: String,
    /// Escrow program ID (base58).
    pub escrow_program_id: String,
    /// Shared HTTP client for RPC calls (avoids per-call allocation).
    pub http_client: reqwest::Client,
}

#[async_trait]
impl PaymentVerifier for EscrowVerifier {
    fn network(&self) -> &str {
        SOLANA_NETWORK
    }

    fn scheme(&self) -> &str {
        "escrow"
    }

    async fn verify_payment(&self, payload: &PaymentPayload) -> Result<VerificationResult, Error> {
        info!(
            network = SOLANA_NETWORK,
            scheme = "escrow",
            resource = %payload.resource.url,
            "verifying escrow deposit"
        );

        // Extract escrow payload
        let escrow_payload = match &payload.payload {
            PayloadData::Escrow(p) => p,
            PayloadData::Direct(_) => {
                return Err(Error::PayloadMismatch(
                    "EscrowVerifier received direct payload; expected escrow".to_string(),
                ));
            }
        };

        // Decode service_id from base64
        use base64::Engine;
        let service_id_bytes = base64::engine::general_purpose::STANDARD
            .decode(&escrow_payload.service_id)
            .map_err(|e| Error::InvalidEncoding(format!("service_id base64: {e}")))?;
        if service_id_bytes.len() != 32 {
            return Err(Error::InvalidTransaction(format!(
                "service_id must be 32 bytes, got {}",
                service_id_bytes.len()
            )));
        }
        let mut service_id = [0u8; 32];
        service_id.copy_from_slice(&service_id_bytes);

        // Decode agent_pubkey from base58
        let agent_pubkey = decode_bs58_pubkey(&escrow_payload.agent_pubkey)
            .map_err(|e| Error::InvalidTransaction(format!("agent_pubkey: {e}")))?;

        // Derive expected escrow PDA
        let program_id = decode_bs58_pubkey(&self.escrow_program_id)
            .map_err(|e| Error::InvalidTransaction(format!("escrow_program_id: {e}")))?;
        let (escrow_pda, _escrow_bump) =
            find_program_address(&[b"escrow", &agent_pubkey, &service_id], &program_id)
                .ok_or_else(|| {
                    Error::InvalidTransaction("failed to derive escrow PDA".to_string())
                })?;

        // Decode and validate the deposit transaction
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&escrow_payload.deposit_tx)
            .map_err(|e| Error::InvalidEncoding(format!("deposit_tx base64: {e}")))?;

        let tx = VersionedTransaction::from_bytes(&tx_bytes).map_err(|e| {
            Error::InvalidTransaction(format!("failed to deserialize deposit tx: {e}"))
        })?;

        // Verify at least one signature exists
        if tx.signatures.is_empty() {
            return Err(Error::InvalidSignature(
                "deposit transaction has no signatures".to_string(),
            ));
        }

        // Verify the first signature cryptographically against the agent key
        let message = tx
            .parse_message()
            .map_err(|e| Error::InvalidTransaction(format!("failed to parse message: {e}")))?;

        if message.account_keys.is_empty() {
            return Err(Error::InvalidSignature(
                "transaction message has no account keys".to_string(),
            ));
        }

        // Verify ed25519 signature
        use ed25519_dalek::{Signature as Ed25519Sig, Verifier, VerifyingKey};
        let signer_pubkey_bytes = &message.account_keys[0].0;
        let sig_bytes = &tx.signatures[0].0;

        let verifying_key = VerifyingKey::from_bytes(signer_pubkey_bytes)
            .map_err(|e| Error::InvalidSignature(format!("invalid signer public key: {e}")))?;

        let ed_sig = Ed25519Sig::from_bytes(sig_bytes);
        verifying_key
            .verify(&tx.message_bytes, &ed_sig)
            .map_err(|e| {
                Error::InvalidSignature(format!("ed25519 signature verification failed: {e}"))
            })?;

        // FIX 4: Verify signer matches agent_pubkey (prevents using someone else's tx)
        if signer_pubkey_bytes != &agent_pubkey {
            return Err(Error::InvalidSignature(
                "signer does not match agent_pubkey".to_string(),
            ));
        }

        // Parse required amount from the payment accept (gateway-set, not client-controlled)
        let required_amount: u64 = payload
            .accepted
            .amount
            .parse()
            .map_err(|_| Error::InvalidTransaction("invalid amount format".to_string()))?;

        // FIX 2: Extract and verify SPL transfer from the deposit transaction.
        // This ensures the deposit tx actually transfers the right amount of the
        // right token to the right destination — never trust client-claimed amounts.
        let transfer = extract_spl_transfer(&message)?;

        // Verify amount >= required
        if transfer.amount < required_amount {
            return Err(Error::InsufficientPayment {
                expected: required_amount,
                actual: transfer.amount,
            });
        }

        // Verify mint is USDC (only TransferChecked provides mint — reject plain Transfer)
        let usdc_mint_bytes = decode_bs58_pubkey(&self.usdc_mint)
            .map_err(|e| Error::InvalidTransaction(format!("usdc_mint config: {e}")))?;
        match transfer.mint {
            Some(mint) => {
                if mint.0 != usdc_mint_bytes {
                    return Err(Error::WrongAsset {
                        expected: self.usdc_mint.clone(),
                        actual: mint.to_string(),
                    });
                }
            }
            None => {
                return Err(Error::InvalidTransaction(
                    "plain SPL Transfer instructions are not accepted; \
                     use TransferChecked (instruction discriminator 12) \
                     so the USDC mint can be verified"
                        .to_string(),
                ));
            }
        }

        // Verify destination is the vault ATA (escrow PDA's token account)
        let vault_ata = derive_ata_address(&escrow_pda, &usdc_mint_bytes).ok_or_else(|| {
            Error::InvalidTransaction("failed to derive vault ATA for verification".to_string())
        })?;
        if transfer.destination.0 != vault_ata {
            return Err(Error::WrongRecipient {
                expected: bs58::encode(&vault_ata).into_string(),
                actual: transfer.destination.to_string(),
            });
        }

        info!(
            required_amount,
            verified_amount = transfer.amount,
            agent = %escrow_payload.agent_pubkey,
            "escrow deposit verification passed"
        );

        Ok(VerificationResult {
            valid: true,
            reason: None,
            verified_amount: Some(transfer.amount),
        })
    }

    async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        info!(
            network = SOLANA_NETWORK,
            scheme = "escrow",
            resource = %payload.resource.url,
            "settling escrow payment (checking deposit confirmation)"
        );

        // Extract escrow payload
        let escrow_payload = match &payload.payload {
            PayloadData::Escrow(p) => p,
            PayloadData::Direct(_) => {
                return Err(Error::PayloadMismatch(
                    "EscrowVerifier received direct payload; expected escrow".to_string(),
                ));
            }
        };

        // Decode deposit tx to extract signature and verified amount
        use base64::Engine;
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&escrow_payload.deposit_tx)
            .map_err(|e| Error::InvalidEncoding(format!("deposit_tx base64: {e}")))?;

        let tx = VersionedTransaction::from_bytes(&tx_bytes).map_err(|e| {
            Error::InvalidTransaction(format!("failed to deserialize deposit tx: {e}"))
        })?;

        if tx.signatures.is_empty() {
            return Err(Error::InvalidSignature(
                "deposit transaction has no signatures".to_string(),
            ));
        }

        // Re-extract the verified deposit amount from the transaction
        let verified_amount = tx
            .parse_message()
            .ok()
            .and_then(|msg| extract_spl_transfer(&msg).ok())
            .map(|t| t.amount);

        // Base58-encode the first signature as the tx identifier
        let sig_b58 = bs58::encode(&tx.signatures[0].0).into_string();

        // Check signature status via RPC (using shared client — FIX 7)
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "getSignatureStatuses",
            "params": [[sig_b58]],
        });

        let response = self
            .http_client
            .post(&self.rpc_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| Error::Rpc(e.to_string()))?;

        let result: serde_json::Value = response
            .json()
            .await
            .map_err(|e| Error::Rpc(e.to_string()))?;

        if let Some(error) = result.get("error") {
            return Err(Error::Rpc(error.to_string()));
        }

        // Check confirmation status
        if let Some(value) = result.get("result").and_then(|r| r.get("value")) {
            if let Some(status) = value.as_array().and_then(|arr| arr.first()) {
                if !status.is_null() {
                    if let Some(err) = status.get("err") {
                        if !err.is_null() {
                            return Err(Error::EscrowNotConfirmed(format!(
                                "deposit transaction failed: {err}"
                            )));
                        }
                    }
                    if let Some(confirmation) =
                        status.get("confirmationStatus").and_then(|s| s.as_str())
                    {
                        if confirmation == "confirmed" || confirmation == "finalized" {
                            info!(
                                signature = %sig_b58,
                                status = confirmation,
                                "escrow deposit confirmed"
                            );
                            return Ok(SettlementResult {
                                success: true,
                                tx_signature: Some(sig_b58),
                                network: SOLANA_NETWORK.to_string(),
                                error: None,
                                verified_amount,
                            });
                        }
                    }
                }
            }
        }

        // Not yet confirmed — return with a note
        Ok(SettlementResult {
            success: true,
            tx_signature: Some(sig_b58),
            network: SOLANA_NETWORK.to_string(),
            error: Some("deposit not yet confirmed — will proceed optimistically".to_string()),
            verified_amount,
        })
    }
}

// ---------------------------------------------------------------------------
// EscrowClaimer — fire-and-forget claim submission
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
            client: reqwest::Client::new(),
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
struct ClaimParams {
    rpc_url: String,
    fee_payer_keypair: [u8; 64],
    escrow_program_id: [u8; 32],
    recipient_wallet: [u8; 32],
    usdc_mint: [u8; 32],
    service_id: [u8; 32],
    agent_pubkey: [u8; 32],
    actual_amount: u64,
    /// Shared HTTP client (avoids per-call allocation).
    client: reqwest::Client,
}

/// Internal: build and submit the claim transaction.
async fn do_claim(params: &ClaimParams) -> Result<(), Error> {
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
    use crate::types::{
        PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, SOLANA_NETWORK,
    };

    #[test]
    fn test_escrow_verifier_creation() {
        let verifier = EscrowVerifier {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "11111111111111111111111111111111".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string(),
            http_client: reqwest::Client::new(),
        };
        assert_eq!(verifier.network(), SOLANA_NETWORK);
        assert_eq!(verifier.scheme(), "escrow");
    }

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

    #[tokio::test]
    async fn test_escrow_verifier_rejects_direct_payload() {
        let verifier = EscrowVerifier {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "11111111111111111111111111111111".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string(),
            http_client: reqwest::Client::new(),
        };

        let payload = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "11111111111111111111111111111111".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: Some("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy".to_string()),
            },
            payload: PayloadData::Direct(SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };

        let result = verifier.verify_payment(&payload).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("expected escrow"));
    }

    #[test]
    fn test_anchor_discriminator() {
        let disc = anchor_discriminator("claim");
        // The discriminator should be 8 bytes from sha256("global:claim")
        assert_eq!(disc.len(), 8);
        // Verify it's deterministic
        assert_eq!(disc, anchor_discriminator("claim"));
        // Different names give different discriminators
        assert_ne!(disc, anchor_discriminator("deposit"));
    }

    #[test]
    fn test_pda_derivation() {
        // Use known inputs and verify we get a deterministic PDA
        let program_id = decode_bs58_pubkey("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy")
            .expect("valid program id");
        let agent = [1u8; 32];
        let service_id = [2u8; 32];

        let result = find_program_address(&[b"escrow", &agent, &service_id], &program_id);
        assert!(result.is_some());

        let (pda, bump) = result.unwrap();
        // PDA must be off the ed25519 curve
        assert!(!is_on_ed25519_curve(&pda));
        // Bump must be a valid u8 (which it is by type — just check it's > 0
        // to confirm the PDA derivation didn't need all 256 attempts)
        let _ = bump; // Bump is a u8, always valid

        // Same inputs → same result
        let (pda2, bump2) =
            find_program_address(&[b"escrow", &agent, &service_id], &program_id).unwrap();
        assert_eq!(pda, pda2);
        assert_eq!(bump, bump2);
    }

    #[test]
    fn test_derive_ata_address() {
        let wallet = [1u8; 32];
        let mint =
            decode_bs58_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").expect("valid mint");

        let ata = derive_ata_address(&wallet, &mint);
        assert!(ata.is_some());

        // Deterministic
        let ata2 = derive_ata_address(&wallet, &mint);
        assert_eq!(ata, ata2);
    }

    #[test]
    fn test_decode_bs58_pubkey_valid() {
        let result = decode_bs58_pubkey("11111111111111111111111111111111");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), [0u8; 32]);
    }

    #[test]
    fn test_decode_bs58_pubkey_invalid() {
        let result = decode_bs58_pubkey("invalid!!!");
        assert!(result.is_err());
    }
}
