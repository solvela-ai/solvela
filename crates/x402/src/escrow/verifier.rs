//! EscrowVerifier — verifies on-chain escrow deposits and settles payments.

use async_trait::async_trait;
use tracing::info;

use crate::solana_types::{ParsedMessage, VersionedTransaction};
use crate::traits::{Error, PaymentVerifier};
use crate::types::{
    PayloadData, PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK,
};

use super::pda::{
    anchor_discriminator, decode_bs58_pubkey, derive_ata_address, find_program_address,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Parse the Anchor `deposit` instruction from a versioned transaction message
/// and return the deposit amount from the instruction data.
///
/// Searches for the top-level instruction whose program is the escrow program,
/// then reads the `amount` field (u64 LE at offset 8 after the 8-byte Anchor
/// discriminator). Returns `None` if no matching instruction is found or the
/// data is malformed.
fn extract_deposit_amount(
    message: &ParsedMessage,
    escrow_program_id: &[u8; 32],
) -> Option<u64> {
    let expected_disc = anchor_discriminator("deposit");
    let deposit_ix = message.instructions.iter().find(|ix| {
        let is_escrow = message
            .account_keys
            .get(ix.program_id_index as usize)
            .map(|key| key.0 == *escrow_program_id)
            .unwrap_or(false);
        is_escrow && ix.data.len() >= 8 && ix.data[..8] == expected_disc
    })?;
    if deposit_ix.data.len() < 16 {
        return None;
    }
    let amount_bytes: [u8; 8] = deposit_ix.data[8..16].try_into().ok()?;
    Some(u64::from_le_bytes(amount_bytes))
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

        // Parse the Anchor `deposit` instruction directly. The actual SPL token
        // transfer happens as a CPI inside the on-chain program, so the
        // client-submitted transaction only contains the top-level deposit call.
        // We verify the instruction is well-formed, targets the right program,
        // encodes the required amount/service_id, and references the expected
        // PDA accounts — the on-chain program enforces the rest.
        let deposit_ix = message
            .instructions
            .iter()
            .find(|ix| {
                message
                    .account_keys
                    .get(ix.program_id_index as usize)
                    .map(|key| key.0 == program_id)
                    .unwrap_or(false)
            })
            .ok_or_else(|| {
                Error::InvalidTransaction(
                    "no escrow program instruction found in deposit tx".to_string(),
                )
            })?;

        // Verify instruction data starts with the Anchor `deposit` discriminator
        // and has the expected length: 8 + 8 + 32 + 8 = 56 bytes.
        let expected_disc = anchor_discriminator("deposit");
        if deposit_ix.data.len() < 56 {
            return Err(Error::InvalidTransaction(format!(
                "deposit instruction data too short: {} bytes (expected 56)",
                deposit_ix.data.len()
            )));
        }
        if deposit_ix.data[..8] != expected_disc {
            return Err(Error::InvalidTransaction(
                "instruction is not an Anchor deposit call".to_string(),
            ));
        }

        // Parse instruction data: discriminator(8) + amount(u64 LE)
        //                         + service_id([u8;32]) + expiry_slot(u64 LE)
        let amount_bytes: [u8; 8] = deposit_ix.data[8..16].try_into().map_err(|_| {
            Error::InvalidTransaction(
                "failed to parse amount from instruction data".to_string(),
            )
        })?;
        let ix_amount = u64::from_le_bytes(amount_bytes);

        let ix_service_id: [u8; 32] = deposit_ix.data[16..48].try_into().map_err(|_| {
            Error::InvalidTransaction(
                "failed to parse service_id from instruction data".to_string(),
            )
        })?;

        // Verify amount >= required (gateway-set, never trust client claims)
        if ix_amount < required_amount {
            return Err(Error::InsufficientPayment {
                expected: required_amount,
                actual: ix_amount,
            });
        }

        // Verify the instruction's service_id matches the payload's service_id
        if ix_service_id != service_id {
            return Err(Error::InvalidTransaction(
                "instruction service_id does not match payload service_id".to_string(),
            ));
        }

        // Verify the deposit instruction's account layout matches what the
        // Anchor program expects. Positions are within the instruction's
        // account indices list, mapped back to message.account_keys.
        // Program layout (from programs/escrow): agent=0, provider=1, mint=2,
        // escrow=3, agent_ata=4, vault=5, token_program=6, ata_program=7, system=8.
        if deposit_ix.accounts.len() < 6 {
            return Err(Error::InvalidTransaction(format!(
                "deposit instruction has {} accounts, expected at least 6",
                deposit_ix.accounts.len()
            )));
        }

        let get_key = |pos: usize| -> Result<[u8; 32], Error> {
            let idx = deposit_ix.accounts[pos] as usize;
            message
                .account_keys
                .get(idx)
                .map(|k| k.0)
                .ok_or_else(|| {
                    Error::InvalidTransaction(format!("account index {idx} out of range"))
                })
        };

        // Position 0: agent (must match payload agent_pubkey / signer)
        let ix_agent = get_key(0)?;
        if ix_agent != agent_pubkey {
            return Err(Error::InvalidSignature(
                "deposit instruction agent account does not match payload agent_pubkey"
                    .to_string(),
            ));
        }

        // Position 2: mint (must be the configured USDC mint)
        let usdc_mint_bytes = decode_bs58_pubkey(&self.usdc_mint)
            .map_err(|e| Error::InvalidTransaction(format!("usdc_mint config: {e}")))?;
        let ix_mint = get_key(2)?;
        if ix_mint != usdc_mint_bytes {
            return Err(Error::WrongAsset {
                expected: self.usdc_mint.clone(),
                actual: bs58::encode(&ix_mint).into_string(),
            });
        }

        // Position 3: escrow PDA (must match the PDA derived from agent + service_id)
        let ix_escrow = get_key(3)?;
        if ix_escrow != escrow_pda {
            return Err(Error::InvalidTransaction(
                "deposit instruction escrow PDA does not match derived PDA".to_string(),
            ));
        }

        // Position 5: vault ATA (must be the ATA owned by the escrow PDA for USDC)
        let vault_ata = derive_ata_address(&escrow_pda, &usdc_mint_bytes).ok_or_else(|| {
            Error::InvalidTransaction("failed to derive vault ATA for verification".to_string())
        })?;
        let ix_vault = get_key(5)?;
        if ix_vault != vault_ata {
            return Err(Error::WrongRecipient {
                expected: bs58::encode(&vault_ata).into_string(),
                actual: bs58::encode(&ix_vault).into_string(),
            });
        }

        info!(
            required_amount,
            verified_amount = ix_amount,
            agent = %escrow_payload.agent_pubkey,
            "escrow deposit verification passed"
        );

        Ok(VerificationResult {
            valid: true,
            reason: None,
            verified_amount: Some(ix_amount),
        })
    }

    async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error> {
        info!(
            network = SOLANA_NETWORK,
            scheme = "escrow",
            resource = %payload.resource.url,
            "settling escrow payment (submitting and confirming deposit)"
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

        // Re-extract the verified deposit amount from the transaction by
        // parsing the Anchor `deposit` instruction (not the SPL transfer, which
        // only exists as a CPI inside the on-chain program execution).
        let program_id_bytes = decode_bs58_pubkey(&self.escrow_program_id)
            .map_err(|e| Error::InvalidTransaction(format!("escrow_program_id: {e}")))?;
        let verified_amount = tx
            .parse_message()
            .ok()
            .and_then(|msg| extract_deposit_amount(&msg, &program_id_bytes));

        // Base58-encode the first signature as the tx identifier
        let sig_b58 = bs58::encode(&tx.signatures[0].0).into_string();

        // Submit the signed deposit tx to Solana RPC.
        // The tx is already signed by the agent; we're just broadcasting it.
        // If the tx is already on-chain (double-submission), sendTransaction returns
        // the same signature — idempotent.
        let polling_sig = match self.send_transaction(&escrow_payload.deposit_tx).await {
            Ok(sig) => {
                info!(signature = %sig, "escrow deposit submitted to Solana RPC");
                sig
            }
            Err(e) => {
                // Check if the error is "already processed" (tx already on-chain) — that's OK
                let err_str = e.to_string();
                if err_str.contains("already been processed") || err_str.contains("already processed") {
                    info!("escrow deposit already on-chain, proceeding to confirmation check");
                    sig_b58.clone()
                } else {
                    tracing::warn!(error = %err_str, "escrow deposit submission failed");
                    return Ok(SettlementResult {
                        success: false,
                        tx_signature: Some(sig_b58),
                        network: SOLANA_NETWORK.to_string(),
                        error: Some(format!("submission failed: {err_str}")),
                        verified_amount,
                    });
                }
            }
        };

        // Poll for confirmation (up to ~10 seconds, 20 attempts at 500ms intervals)
        for attempt in 0..20u32 {
            if attempt > 0 {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }

            let body = serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "getSignatureStatuses",
                "params": [[polling_sig]],
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

            if let Some(value) = result.get("result").and_then(|r| r.get("value")) {
                if let Some(status) = value.as_array().and_then(|arr| arr.first()) {
                    if !status.is_null() {
                        if let Some(err) = status.get("err") {
                            if !err.is_null() {
                                return Err(Error::EscrowNotConfirmed(format!(
                                    "deposit transaction failed on-chain: {err}"
                                )));
                            }
                        }
                        if let Some(confirmation) =
                            status.get("confirmationStatus").and_then(|s| s.as_str())
                        {
                            match confirmation {
                                "confirmed" | "finalized" => {
                                    info!(
                                        signature = %polling_sig,
                                        status = confirmation,
                                        attempt,
                                        "escrow deposit confirmed"
                                    );
                                    return Ok(SettlementResult {
                                        success: true,
                                        tx_signature: Some(polling_sig),
                                        network: SOLANA_NETWORK.to_string(),
                                        error: None,
                                        verified_amount,
                                    });
                                }
                                "processed" => {
                                    info!(
                                        signature = %polling_sig,
                                        status = confirmation,
                                        attempt,
                                        "escrow deposit processed (optimistic settle)"
                                    );
                                    return Ok(SettlementResult {
                                        success: true,
                                        tx_signature: Some(polling_sig),
                                        network: SOLANA_NETWORK.to_string(),
                                        error: None,
                                        verified_amount,
                                    });
                                }
                                _ => {
                                    // Unknown confirmation status — continue polling
                                }
                            }
                        }
                    }
                }
            }
        }

        // Exceeded poll budget — deposit not confirmed within ~10 seconds
        Ok(SettlementResult {
            success: false,
            tx_signature: Some(sig_b58),
            network: SOLANA_NETWORK.to_string(),
            error: Some("escrow deposit not confirmed within 10 seconds".to_string()),
            verified_amount,
        })
    }
}

impl EscrowVerifier {
    /// Broadcast a signed transaction to the Solana cluster.
    async fn send_transaction(&self, base64_tx: &str) -> Result<String, Error> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "sendTransaction",
            "params": [
                base64_tx,
                {
                    "encoding": "base64",
                    "skipPreflight": false,
                    "preflightCommitment": "confirmed",
                    "maxRetries": 3,
                }
            ],
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

        result
            .get("result")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::Rpc("sendTransaction did not return a signature".to_string())
            })
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{
        EscrowPayload, PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload,
        SOLANA_NETWORK,
    };

    #[test]
    fn test_escrow_verifier_creation() {
        let verifier = EscrowVerifier {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "11111111111111111111111111111111".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        };
        assert_eq!(verifier.network(), SOLANA_NETWORK);
        assert_eq!(verifier.scheme(), "escrow");
    }

    #[tokio::test]
    async fn test_escrow_verifier_rejects_direct_payload() {
        let verifier = EscrowVerifier {
            rpc_url: "https://api.devnet.solana.com".to_string(),
            recipient_wallet: "11111111111111111111111111111111".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
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
                escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
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

    #[tokio::test]
    async fn test_escrow_verifier_accepts_valid_deposit_tx() {
        use crate::escrow::deposit::{build_deposit_tx, DepositParams};

        // Build a valid deposit tx using the same builder the CLI uses
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        let agent_keypair_b58 = bs58::encode(&full).into_string();

        let service_id = [7u8; 32];
        let params = DepositParams {
            agent_keypair_b58,
            provider_wallet_b58: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            usdc_mint_b58: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id_b58: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            amount: 5000,
            service_id,
            expiry_slot: 999_999_999,
            recent_blockhash: [0u8; 32],
        };
        let deposit_tx_b64 = build_deposit_tx(&params).expect("tx build");

        let agent_pubkey_b58 = bs58::encode(verifying_key.as_bytes()).into_string();

        use base64::Engine;
        let service_id_b64 = base64::engine::general_purpose::STANDARD.encode(service_id);

        let payload = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "5000".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
            },
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx: deposit_tx_b64,
                service_id: service_id_b64,
                agent_pubkey: agent_pubkey_b58,
            }),
        };

        let verifier = EscrowVerifier {
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            recipient_wallet: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            http_client: reqwest::Client::new(),
        };

        // verify_payment doesn't hit the network (only settle_payment does)
        let result = verifier.verify_payment(&payload).await;
        assert!(
            result.is_ok(),
            "verification should succeed: {:?}",
            result.err()
        );
        let vr = result.unwrap();
        assert!(vr.valid);
        assert_eq!(vr.verified_amount, Some(5000));
    }

    /// Helper: build a base verifier and the agent keypair used across negative tests.
    fn make_verifier() -> EscrowVerifier {
        EscrowVerifier {
            rpc_url: "https://api.mainnet-beta.solana.com".to_string(),
            recipient_wallet: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
            http_client: reqwest::Client::new(),
        }
    }

    /// Build a deposit tx and return (deposit_tx_b64, agent_pubkey_b58) for the given parameters.
    fn build_test_tx(
        service_id: [u8; 32],
        amount: u64,
        escrow_program_id_b58: &str,
    ) -> (String, String) {
        use crate::escrow::deposit::{build_deposit_tx, DepositParams};
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        let agent_keypair_b58 = bs58::encode(&full).into_string();
        let agent_pubkey_b58 = bs58::encode(verifying_key.as_bytes()).into_string();

        let params = DepositParams {
            agent_keypair_b58,
            provider_wallet_b58: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            usdc_mint_b58: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            escrow_program_id_b58: escrow_program_id_b58.to_string(),
            amount,
            service_id,
            expiry_slot: 999_999_999,
            recent_blockhash: [0u8; 32],
        };
        let deposit_tx_b64 = build_deposit_tx(&params).expect("tx build");
        (deposit_tx_b64, agent_pubkey_b58)
    }

    #[tokio::test]
    async fn test_escrow_verifier_rejects_mismatched_service_id() {
        use base64::Engine;

        // Tx built with service_id=[7;32], but payload claims service_id=[8;32]
        let tx_service_id = [7u8; 32];
        let payload_service_id = [8u8; 32];
        let (deposit_tx_b64, agent_pubkey_b58) =
            build_test_tx(tx_service_id, 5000, "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");

        let service_id_b64 =
            base64::engine::general_purpose::STANDARD.encode(payload_service_id);

        let payload = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "5000".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: Some(
                    "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
                ),
            },
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx: deposit_tx_b64,
                service_id: service_id_b64,
                agent_pubkey: agent_pubkey_b58,
            }),
        };

        let result = make_verifier().verify_payment(&payload).await;
        assert!(result.is_err(), "expected error for mismatched service_id");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("service_id"),
            "error should mention service_id, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_escrow_verifier_rejects_insufficient_amount() {
        use base64::Engine;

        // Tx encodes amount=1000, but gateway requires 5000
        let service_id = [7u8; 32];
        let (deposit_tx_b64, agent_pubkey_b58) =
            build_test_tx(service_id, 1000, "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");
        let service_id_b64 = base64::engine::general_purpose::STANDARD.encode(service_id);

        let payload = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "5000".to_string(), // gateway requires 5000
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: Some(
                    "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
                ),
            },
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx: deposit_tx_b64,
                service_id: service_id_b64,
                agent_pubkey: agent_pubkey_b58,
            }),
        };

        let result = make_verifier().verify_payment(&payload).await;
        assert!(result.is_err(), "expected InsufficientPayment error");
        match result.unwrap_err() {
            Error::InsufficientPayment { expected, actual } => {
                assert_eq!(expected, 5000);
                assert_eq!(actual, 1000);
            }
            other => panic!("expected InsufficientPayment, got: {other}"),
        }
    }

    #[tokio::test]
    async fn test_escrow_verifier_rejects_wrong_escrow_program() {
        use base64::Engine;

        // Tx built against a different escrow program than the verifier is configured with.
        // Use a valid-length base58 pubkey that is distinct from the configured one.
        let wrong_program = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
        let service_id = [7u8; 32];
        let (deposit_tx_b64, agent_pubkey_b58) = build_test_tx(service_id, 5000, wrong_program);
        let service_id_b64 = base64::engine::general_purpose::STANDARD.encode(service_id);

        let payload = PaymentPayload {
            x402_version: 2,
            resource: Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "5000".to_string(),
                asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
                pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: Some(
                    "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string(),
                ),
            },
            payload: PayloadData::Escrow(EscrowPayload {
                deposit_tx: deposit_tx_b64,
                service_id: service_id_b64,
                agent_pubkey: agent_pubkey_b58,
            }),
        };

        // Verifier uses the canonical escrow program; tx targets wrong_program → no instruction found
        let result = make_verifier().verify_payment(&payload).await;
        assert!(result.is_err(), "expected error for wrong escrow program");
        let err = result.unwrap_err();
        assert!(
            err.to_string().contains("no escrow program instruction found"),
            "error should mention missing instruction, got: {err}"
        );
    }
}
