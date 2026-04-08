//! EscrowVerifier — verifies on-chain escrow deposits and settles payments.

use async_trait::async_trait;
use tracing::info;

use crate::solana_types::VersionedTransaction;
use crate::spl_transfer::extract_spl_transfer;
use crate::traits::{Error, PaymentVerifier};
use crate::types::{
    PayloadData, PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK,
};

use super::pda::{decode_bs58_pubkey, derive_ata_address, find_program_address};

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
                        match confirmation {
                            "confirmed" | "finalized" => {
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
                            "processed" => {
                                info!(
                                    signature = %sig_b58,
                                    status = confirmation,
                                    "escrow deposit processed (optimistic settle)"
                                );
                                return Ok(SettlementResult {
                                    success: true,
                                    tx_signature: Some(sig_b58),
                                    network: SOLANA_NETWORK.to_string(),
                                    error: None,
                                    verified_amount,
                                });
                            }
                            _ => {
                                // Unknown confirmation status — reject
                            }
                        }
                    }
                }
            }
        }

        // Not yet confirmed — reject to prevent servicing unconfirmed deposits
        Ok(SettlementResult {
            success: false,
            tx_signature: Some(sig_b58),
            network: SOLANA_NETWORK.to_string(),
            error: Some(
                "escrow deposit not yet confirmed on-chain; \
                 transaction must reach at least \"processed\" status"
                    .to_string(),
            ),
            verified_amount,
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
        PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, SOLANA_NETWORK,
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
}
