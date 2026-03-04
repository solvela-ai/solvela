use std::str::FromStr;

use async_trait::async_trait;
use tracing::{debug, info, warn};

use crate::nonce_pool::NoncePool;
use crate::solana_types::{derive_ata, ParsedMessage, Pubkey, VersionedTransaction};
use crate::spl_transfer::extract_spl_transfer;
use crate::traits::{Error, PaymentVerifier};
use crate::types::{
    PayloadData, PaymentPayload, SettlementResult, VerificationResult, SOLANA_NETWORK,
};

/// Solana-specific x402 payment verifier.
///
/// Verifies and settles USDC-SPL payments on Solana by introspecting
/// pre-signed versioned transactions. Uses reqwest for JSON-RPC calls
/// instead of solana-client (which has dependency conflicts with solana-sdk 2.x).
///
/// Supports both:
/// - Recent blockhash transactions (normal, expire ~60 seconds)
/// - Durable nonce transactions (pre-signed, never expire)
pub struct SolanaVerifier {
    /// Solana RPC endpoint URL.
    rpc_url: String,
    /// HTTP client for JSON-RPC calls.
    http_client: reqwest::Client,
    /// The recipient wallet pubkey (gateway's USDC recipient).
    recipient: Pubkey,
    /// The USDC mint pubkey.
    usdc_mint: Pubkey,
    /// Optional durable nonce pool for validating nonce-based transactions.
    /// When `None`, all nonce-based transactions are accepted (trusted to network).
    nonce_pool: Option<std::sync::Arc<NoncePool>>,
}

impl SolanaVerifier {
    /// Create a new Solana payment verifier.
    ///
    /// # Arguments
    /// * `rpc_url` - Solana RPC endpoint URL
    /// * `recipient` - The gateway's USDC recipient wallet address
    /// * `usdc_mint` - The USDC-SPL mint address
    pub fn new(rpc_url: &str, recipient: &str, usdc_mint: &str) -> Result<Self, Error> {
        let recipient =
            Pubkey::from_str(recipient).map_err(|e| Error::InvalidTransaction(e.to_string()))?;

        let usdc_mint =
            Pubkey::from_str(usdc_mint).map_err(|e| Error::InvalidTransaction(e.to_string()))?;

        Ok(Self {
            rpc_url: rpc_url.to_string(),
            http_client: reqwest::Client::new(),
            recipient,
            usdc_mint,
            nonce_pool: None,
        })
    }

    /// Attach a durable nonce pool for validating nonce-based transactions.
    ///
    /// When a nonce pool is attached, the verifier will check that the nonce account
    /// referenced in the `AdvanceNonceAccount` instruction belongs to the pool.
    pub fn with_nonce_pool(mut self, pool: std::sync::Arc<NoncePool>) -> Self {
        self.nonce_pool = Some(pool);
        self
    }

    /// Detect whether the transaction uses a durable nonce.
    ///
    /// A nonce transaction is identified by its FIRST instruction being a
    /// SystemProgram `AdvanceNonceAccount` instruction:
    /// - Program ID: `11111111111111111111111111111111` (SystemProgram, all zeros)
    /// - Instruction data: starts with `[4, 0, 0, 0]` (AdvanceNonceAccount discriminator)
    ///
    /// Returns the nonce account pubkey if found, `None` if the tx uses a recent blockhash.
    pub fn is_nonce_transaction(message: &ParsedMessage) -> Option<Pubkey> {
        let first_ix = message.instructions.first()?;

        // The program must be the SystemProgram (index into account_keys)
        let program_idx = first_ix.program_id_index as usize;
        let program = message.account_keys.get(program_idx)?;
        if *program != Pubkey::SYSTEM_PROGRAM {
            return None;
        }

        // AdvanceNonceAccount discriminator is [4, 0, 0, 0]
        if first_ix.data.len() < 4 || first_ix.data[0..4] != [4, 0, 0, 0] {
            return None;
        }

        // The nonce account is the first account referenced by this instruction
        let nonce_account_idx = *first_ix.accounts.first()? as usize;
        let nonce_account = message.account_keys.get(nonce_account_idx)?;
        Some(*nonce_account)
    }

    /// Decode a base64-encoded versioned transaction and perform cryptographic validation.
    ///
    /// Validates:
    /// 1. Base64 decoding succeeds
    /// 2. Transaction deserializes correctly
    /// 3. At least one signature is present
    /// 4. The first signature cryptographically verifies over the message bytes
    ///    using the first account key (fee payer / primary signer) as the public key
    fn decode_and_validate_transaction(
        &self,
        base64_tx: &str,
    ) -> Result<VersionedTransaction, Error> {
        use base64::Engine;
        use ed25519_dalek::{Signature as Ed25519Sig, Verifier, VerifyingKey};

        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(base64_tx)
            .map_err(|e| Error::InvalidEncoding(e.to_string()))?;

        let tx = VersionedTransaction::from_bytes(&tx_bytes).map_err(|e| {
            Error::InvalidTransaction(format!("failed to deserialize transaction: {e}"))
        })?;

        // Verify the transaction has at least one signature
        if tx.signatures.is_empty() {
            return Err(Error::InvalidSignature(
                "transaction has no signatures".to_string(),
            ));
        }

        // Cryptographically verify the first signature against the message bytes.
        // The first signature must be made by the first account key (fee payer / signer).
        // Parse the message to extract account keys.
        let message = tx
            .parse_message()
            .map_err(|e| Error::InvalidTransaction(format!("failed to parse message: {e}")))?;

        if message.account_keys.is_empty() {
            return Err(Error::InvalidSignature(
                "transaction message has no account keys".to_string(),
            ));
        }

        // account_keys[0] is always the fee payer / primary signer
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

        Ok(tx)
    }

    /// Simulate a transaction via RPC to validate it would succeed.
    ///
    /// For nonce-based transactions, `replace_recent_blockhash` must be `false`
    /// because the nonce value IS in the blockhash field — replacing it would
    /// invalidate the `AdvanceNonceAccount` instruction.
    async fn simulate_transaction(
        &self,
        base64_tx: &str,
        replace_recent_blockhash: bool,
    ) -> Result<(), Error> {
        let result = self
            .rpc_request(
                "simulateTransaction",
                serde_json::json!([
                    base64_tx,
                    {
                        "encoding": "base64",
                        "commitment": "confirmed",
                        "replaceRecentBlockhash": replace_recent_blockhash
                    }
                ]),
            )
            .await?;

        // Check if simulation returned an error
        if let Some(value) = result.get("result") {
            if let Some(err) = value.get("err") {
                if !err.is_null() {
                    return Err(Error::SimulationFailed(err.to_string()));
                }
            }
        }

        Ok(())
    }

    /// Broadcast a signed transaction to the cluster.
    async fn send_transaction(&self, base64_tx: &str) -> Result<String, Error> {
        let result = self
            .rpc_request(
                "sendTransaction",
                serde_json::json!([
                    base64_tx,
                    {
                        "encoding": "base64",
                        "skipPreflight": false,
                        "preflightCommitment": "confirmed"
                    }
                ]),
            )
            .await?;

        result
            .get("result")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                Error::SettlementFailed("sendTransaction did not return a signature".to_string())
            })
    }

    /// Poll for transaction confirmation with a timeout.
    async fn confirm_transaction(&self, signature: &str, timeout_secs: u64) -> Result<(), Error> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);
        let poll_interval = std::time::Duration::from_millis(500);

        loop {
            if start.elapsed() > timeout {
                return Err(Error::Timeout);
            }

            let result = self
                .rpc_request("getSignatureStatuses", serde_json::json!([[signature]]))
                .await?;

            if let Some(value) = result.get("result").and_then(|r| r.get("value")) {
                if let Some(status) = value.as_array().and_then(|arr| arr.first()) {
                    if !status.is_null() {
                        // Check for transaction error
                        if let Some(err) = status.get("err") {
                            if !err.is_null() {
                                return Err(Error::SettlementFailed(format!(
                                    "transaction failed: {err}"
                                )));
                            }
                        }

                        // Check confirmation status
                        if let Some(confirmation) =
                            status.get("confirmationStatus").and_then(|s| s.as_str())
                        {
                            if confirmation == "confirmed" || confirmation == "finalized" {
                                info!(signature, status = confirmation, "transaction confirmed");
                                return Ok(());
                            }
                        }
                    }
                }
            }

            tokio::time::sleep(poll_interval).await;
        }
    }

    /// Send a JSON-RPC request to the Solana cluster.
    async fn rpc_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, Error> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params,
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

        Ok(result)
    }
}

#[async_trait]
impl PaymentVerifier for SolanaVerifier {
    fn network(&self) -> &str {
        SOLANA_NETWORK
    }

    fn scheme(&self) -> &str {
        "exact"
    }

    async fn verify_payment(&self, payload: &PaymentPayload) -> Result<VerificationResult, Error> {
        info!(
            network = SOLANA_NETWORK,
            resource = %payload.resource.url,
            "verifying solana payment"
        );

        // Extract the direct payload variant — reject escrow payloads
        let solana_payload = match &payload.payload {
            PayloadData::Direct(p) => p,
            PayloadData::Escrow(_) => {
                return Err(Error::PayloadMismatch(
                    "SolanaVerifier received escrow payload; expected direct".to_string(),
                ));
            }
        };

        // Step 1: Decode and validate transaction structure
        let tx = self.decode_and_validate_transaction(&solana_payload.transaction)?;

        // Step 2: Parse required amount from the payment accept
        let required_amount: u64 = payload
            .accepted
            .amount
            .parse()
            .map_err(|_| Error::InvalidTransaction("invalid amount format".to_string()))?;

        // Step 3: Parse message and extract SPL transfer
        let message = tx
            .parse_message()
            .map_err(|e| Error::InvalidTransaction(format!("failed to parse message: {e}")))?;

        let transfer = extract_spl_transfer(&message)?;

        // Step 4: Verify destination matches the recipient's ATA for the USDC mint.
        //
        // SPL token transfers go between token accounts (ATAs), not wallet addresses.
        // The destination in the transaction is an ATA — we must derive the expected
        // ATA from our recipient wallet + USDC mint and compare, not compare against
        // the raw wallet pubkey directly.
        let expected_ata = derive_ata(&self.recipient, &self.usdc_mint, &Pubkey::TOKEN_PROGRAM_ID)
            .ok_or_else(|| {
                Error::InvalidTransaction("failed to derive expected ATA address".to_string())
            })?;

        if transfer.destination != expected_ata {
            // Also accept if destination == raw recipient (some older integrations
            // use the wallet address directly for wrapped SOL / native accounts).
            if transfer.destination != self.recipient {
                return Err(Error::WrongRecipient {
                    expected: expected_ata.to_string(),
                    actual: transfer.destination.to_string(),
                });
            }
        }

        // Step 5: Verify mint.
        // - TransferChecked includes the mint in the instruction — verify it.
        // - Plain Transfer (discriminator 3) does NOT include the mint, so we
        //   cannot verify which token is being transferred. Reject it outright
        //   to prevent payments made with worthless SPL tokens instead of USDC.
        match transfer.mint {
            Some(mint) => {
                if mint != self.usdc_mint {
                    return Err(Error::WrongAsset {
                        expected: self.usdc_mint.to_string(),
                        actual: mint.to_string(),
                    });
                }
            }
            None => {
                // Plain Transfer instruction — mint is unverifiable.
                // Only accept TransferChecked (discriminator 12) for payments.
                return Err(Error::InvalidTransaction(
                    "plain SPL Transfer instructions are not accepted; \
                     use TransferChecked (instruction discriminator 12) \
                     so the USDC mint can be verified on-chain"
                        .to_string(),
                ));
            }
        }

        // Step 6: Verify amount >= required
        if transfer.amount < required_amount {
            return Err(Error::InsufficientPayment {
                expected: required_amount,
                actual: transfer.amount,
            });
        }

        // Step 7: Detect nonce-based transactions and simulate accordingly.
        //
        // For durable nonce transactions:
        // - The first instruction MUST be AdvanceNonceAccount (SystemProgram ix #4)
        // - The blockhash field contains the nonce value — do NOT replace it
        // - If a nonce pool is configured, verify the nonce account is in the pool
        // - If no pool, trust the Solana network to reject invalid nonces
        let is_nonce = Self::is_nonce_transaction(&message);
        if let Some(nonce_account) = is_nonce {
            debug!(
                nonce_account = %nonce_account,
                "detected durable nonce transaction — skipping blockhash recency check"
            );

            // If a nonce pool is configured, verify the nonce account is registered
            if let Some(pool) = &self.nonce_pool {
                let nonce_account_b58 = nonce_account.to_string();
                let is_known = pool
                    .entries_iter()
                    .any(|e| e.nonce_account == nonce_account_b58);
                if !is_known {
                    return Err(Error::InvalidTransaction(format!(
                        "nonce account {nonce_account_b58} is not registered in the gateway nonce pool"
                    )));
                }
            }

            // Simulate WITHOUT replacing the blockhash (nonce tx must keep its nonce value)
            self.simulate_transaction(&solana_payload.transaction, false)
                .await?;
        } else {
            // Normal recent-blockhash transaction: replace for simulation safety
            self.simulate_transaction(&solana_payload.transaction, true)
                .await?;
        }

        info!(
            required_amount,
            actual_amount = transfer.amount,
            recipient = %self.recipient,
            "payment verification passed"
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
            resource = %payload.resource.url,
            "settling solana payment"
        );

        // Extract the direct payload variant
        let solana_payload = match &payload.payload {
            PayloadData::Direct(p) => p,
            PayloadData::Escrow(_) => {
                return Err(Error::PayloadMismatch(
                    "SolanaVerifier received escrow payload; expected direct".to_string(),
                ));
            }
        };

        // Validate the transaction can be decoded
        let _tx = self.decode_and_validate_transaction(&solana_payload.transaction)?;

        // Broadcast the transaction
        let signature = self
            .send_transaction(&solana_payload.transaction)
            .await
            .map_err(|e| Error::SettlementFailed(format!("send failed: {e}")))?;

        info!(signature = %signature, "transaction sent, waiting for confirmation");

        // Wait for confirmation (30 second timeout)
        match self.confirm_transaction(&signature, 30).await {
            Ok(()) => {
                info!(signature = %signature, "settlement confirmed");
                Ok(SettlementResult {
                    success: true,
                    tx_signature: Some(signature),
                    network: SOLANA_NETWORK.to_string(),
                    error: None,
                    verified_amount: None,
                })
            }
            Err(e) => {
                warn!(
                    signature = %signature,
                    error = %e,
                    "settlement confirmation failed"
                );
                Ok(SettlementResult {
                    success: false,
                    tx_signature: Some(signature),
                    network: SOLANA_NETWORK.to_string(),
                    error: Some(e.to_string()),
                    verified_amount: None,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_types::{ParsedMessage, Pubkey};

    // -----------------------------------------------------------------------
    // Helpers for building test transactions
    // -----------------------------------------------------------------------

    /// Recipient used in tests (system program address for simplicity).
    const TEST_RECIPIENT: &str = "11111111111111111111111111111111";
    /// USDC mint used in tests.
    const TEST_USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";

    fn test_verifier() -> SolanaVerifier {
        SolanaVerifier::new(
            "https://api.devnet.solana.com",
            TEST_RECIPIENT,
            TEST_USDC_MINT,
        )
        .expect("failed to create test verifier")
    }

    /// Build a minimal legacy message with an SPL Transfer instruction.
    ///
    /// `signer` is placed as `account_keys[0]` (fee payer / primary signer).
    /// Account layout: [signer/source, destination, authority, token_program]
    fn build_spl_transfer_message(signer: &Pubkey, destination: &Pubkey, amount: u64) -> Vec<u8> {
        let authority =
            Pubkey::from_str("HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH").expect("valid pubkey");
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        // signer doubles as source token account for test simplicity
        let account_keys = [*signer, *destination, authority, token_program];

        let mut ix_data = vec![3u8]; // Transfer discriminator
        ix_data.extend_from_slice(&amount.to_le_bytes());

        build_legacy_message_raw(
            &account_keys,
            &[(3, &[0, 1, 2], &ix_data)], // program_id_index=3 (token_program)
        )
    }

    /// Build a minimal legacy message with an SPL TransferChecked instruction.
    ///
    /// `signer` is placed as `account_keys[0]` (fee payer / primary signer).
    /// Account layout: [signer/source, mint, destination, authority, token_program]
    fn build_spl_transfer_checked_message(
        signer: &Pubkey,
        destination: &Pubkey,
        mint: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Vec<u8> {
        let authority =
            Pubkey::from_str("HN7cABqLq46Es1jh92dQQisAq662SmxELLLsHHe4YWrH").expect("valid pubkey");
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        // signer doubles as source token account for test simplicity
        let account_keys = [*signer, *mint, *destination, authority, token_program];

        let mut ix_data = vec![12u8]; // TransferChecked discriminator
        ix_data.extend_from_slice(&amount.to_le_bytes());
        ix_data.push(decimals);

        build_legacy_message_raw(
            &account_keys,
            &[(4, &[0, 1, 2, 3], &ix_data)], // program_id_index=4 (token_program)
        )
    }

    /// Low-level helper: build a legacy message byte vector.
    fn build_legacy_message_raw(
        account_keys: &[Pubkey],
        instructions: &[(u8, &[u8], &[u8])],
    ) -> Vec<u8> {
        let mut msg = vec![
            1,                        // num_required_signatures
            0,                        // num_readonly_signed
            1,                        // num_readonly_unsigned
            account_keys.len() as u8, // compact-u16
        ];

        // Account keys
        for key in account_keys {
            msg.extend_from_slice(&key.0);
        }

        // Recent blockhash
        msg.extend_from_slice(&[0u8; 32]);

        // Instructions
        msg.push(instructions.len() as u8); // compact-u16
        for (pid_idx, accounts, data) in instructions {
            msg.push(*pid_idx);
            msg.push(accounts.len() as u8);
            msg.extend_from_slice(accounts);
            msg.push(data.len() as u8);
            msg.extend_from_slice(data);
        }

        msg
    }

    /// Wrap a message in a full transaction with a real ed25519 signature.
    ///
    /// Uses a deterministic test signing key (seed = `[1u8; 32]`) so the
    /// signature is valid and `decode_and_validate_transaction` passes.
    /// The caller must ensure `account_keys[0]` in the message matches the
    /// verifying key produced by this seed (see `test_signer_pubkey()`).
    fn wrap_in_transaction(message: &[u8]) -> Vec<u8> {
        use ed25519_dalek::{Signer, SigningKey};

        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let signature = signing_key.sign(message);

        let mut tx_data = Vec::new();
        tx_data.push(0x01); // compact-u16: 1 signature
        tx_data.extend_from_slice(&signature.to_bytes());
        tx_data.extend_from_slice(message);
        tx_data
    }

    /// Returns the 32-byte verifying key (pubkey) for the test signing key seed `[1u8; 32]`.
    fn test_signer_pubkey() -> Pubkey {
        use ed25519_dalek::SigningKey;
        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        Pubkey(signing_key.verifying_key().to_bytes())
    }

    fn encode_base64(data: &[u8]) -> String {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD.encode(data)
    }

    // -----------------------------------------------------------------------
    // Verifier creation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_solana_verifier_creation() {
        let verifier = test_verifier();
        assert_eq!(verifier.network(), SOLANA_NETWORK);
    }

    #[test]
    fn test_solana_verifier_invalid_recipient() {
        let result = SolanaVerifier::new(
            "https://api.devnet.solana.com",
            "not-a-valid-pubkey",
            TEST_USDC_MINT,
        );
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // SPL Transfer extraction tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_extract_spl_transfer_basic() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let msg_bytes = build_spl_transfer_message(&signer, &recipient, 5000);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let transfer = extract_spl_transfer(&parsed).unwrap();
        assert_eq!(transfer.destination, recipient);
        assert_eq!(transfer.amount, 5000);
        assert!(transfer.mint.is_none());
    }

    #[test]
    fn test_extract_spl_transfer_checked() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let usdc_mint = Pubkey::from_str(TEST_USDC_MINT).unwrap();
        let msg_bytes =
            build_spl_transfer_checked_message(&signer, &recipient, &usdc_mint, 10000, 6);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let transfer = extract_spl_transfer(&parsed).unwrap();
        assert_eq!(transfer.destination, recipient);
        assert_eq!(transfer.amount, 10000);
        assert_eq!(transfer.mint, Some(usdc_mint));
    }

    #[test]
    fn test_extract_spl_transfer_no_token_instruction() {
        // Build a message with only a system program instruction (no SPL token)
        let keys = [Pubkey::SYSTEM_PROGRAM, Pubkey::SYSTEM_PROGRAM];
        let msg_bytes = build_legacy_message_raw(&keys, &[(0, &[1], &[0x02, 0x00])]);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let result = extract_spl_transfer(&parsed);
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // verify_payment offline tests (no RPC - will fail at simulation step)
    // -----------------------------------------------------------------------

    #[test]
    fn test_wrong_recipient_detected() {
        // Build a transfer to a different recipient (not the verifier's expected recipient)
        let signer = test_signer_pubkey();
        let wrong_recipient =
            Pubkey::from_str("9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz").unwrap();
        let msg_bytes = build_spl_transfer_message(&signer, &wrong_recipient, 5000);
        let tx_bytes = wrap_in_transaction(&msg_bytes);
        let parsed_msg = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let verifier = test_verifier();

        // The verifier's recipient is TEST_RECIPIENT (system program)
        let transfer = extract_spl_transfer(&parsed_msg).unwrap();
        assert_ne!(transfer.destination, verifier.recipient);

        // Also test decode_and_validate path — signature must be valid
        let base64_tx = encode_base64(&tx_bytes);
        let tx = verifier
            .decode_and_validate_transaction(&base64_tx)
            .unwrap();
        let message = tx.parse_message().unwrap();
        let transfer = extract_spl_transfer(&message).unwrap();
        assert_ne!(transfer.destination, verifier.recipient);
    }

    #[test]
    fn test_insufficient_payment_detected() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let msg_bytes = build_spl_transfer_message(&signer, &recipient, 100); // only 100 atomic units
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let transfer = extract_spl_transfer(&parsed).unwrap();
        let required_amount: u64 = 5000;

        assert!(
            transfer.amount < required_amount,
            "expected amount {} < required {}",
            transfer.amount,
            required_amount
        );
    }

    #[test]
    fn test_wrong_mint_detected() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let wrong_mint = Pubkey::from_str("So11111111111111111111111111111111111111112").unwrap();
        let msg_bytes =
            build_spl_transfer_checked_message(&signer, &recipient, &wrong_mint, 5000, 9);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let verifier = test_verifier();
        let transfer = extract_spl_transfer(&parsed).unwrap();

        assert!(transfer.mint.is_some());
        assert_ne!(transfer.mint.unwrap(), verifier.usdc_mint);
    }

    #[test]
    fn test_decode_and_validate_no_signatures() {
        // Build a transaction with 0 signatures
        let keys = [Pubkey::SYSTEM_PROGRAM];
        let msg_bytes = build_legacy_message_raw(&keys, &[(0, &[], &[])]);
        let mut tx_data = Vec::new();
        tx_data.push(0x00); // compact-u16: 0 signatures
        tx_data.extend_from_slice(&msg_bytes);

        let base64_tx = encode_base64(&tx_data);
        let verifier = test_verifier();
        let result = verifier.decode_and_validate_transaction(&base64_tx);
        assert!(result.is_err());
    }

    #[test]
    fn test_decode_and_validate_invalid_base64() {
        let verifier = test_verifier();
        let result = verifier.decode_and_validate_transaction("not-valid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_full_transfer_extraction_via_transaction() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let msg_bytes = build_spl_transfer_message(&signer, &recipient, 7500);
        let tx_bytes = wrap_in_transaction(&msg_bytes);
        let base64_tx = encode_base64(&tx_bytes);

        let verifier = test_verifier();
        let tx = verifier
            .decode_and_validate_transaction(&base64_tx)
            .unwrap();
        let message = tx.parse_message().unwrap();
        let transfer = extract_spl_transfer(&message).unwrap();

        assert_eq!(transfer.destination, recipient);
        assert_eq!(transfer.amount, 7500);
    }

    // -----------------------------------------------------------------------
    // is_nonce_transaction tests
    // -----------------------------------------------------------------------

    /// Build a message with AdvanceNonceAccount as the first instruction,
    /// followed by an SPL TransferChecked instruction.
    ///
    /// Account layout:
    ///   [0] signer/fee-payer (authority)
    ///   [1] nonce account
    ///   [2] system program
    ///   [3] mint
    ///   [4] destination
    ///   [5] token program
    fn build_nonce_tx_message(
        signer: &Pubkey,
        nonce_account: &Pubkey,
        destination: &Pubkey,
        mint: &Pubkey,
        amount: u64,
        decimals: u8,
    ) -> Vec<u8> {
        let system_program = Pubkey::SYSTEM_PROGRAM;
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        let account_keys = [
            *signer,
            *nonce_account,
            system_program,
            *mint,
            *destination,
            token_program,
        ];

        // AdvanceNonceAccount discriminator = [4, 0, 0, 0]
        let advance_nonce_data: &[u8] = &[4, 0, 0, 0];
        // AdvanceNonceAccount accounts: nonce_account (idx 1), signer/authority (idx 0), system_program (idx 2)
        let advance_nonce_accounts: &[u8] = &[1, 0, 2];

        // TransferChecked: source=signer(0), mint=mint(3), dest=dest(4), authority=signer(0), program=token_program(5)
        let mut transfer_data = vec![12u8]; // TransferChecked discriminator
        transfer_data.extend_from_slice(&amount.to_le_bytes());
        transfer_data.push(decimals);
        let transfer_accounts: &[u8] = &[0, 3, 4, 0]; // source, mint, dest, authority

        build_legacy_message_raw(
            &account_keys,
            &[
                (2, advance_nonce_accounts, advance_nonce_data), // SystemProgram at idx 2
                (5, transfer_accounts, &transfer_data),          // TokenProgram at idx 5
            ],
        )
    }

    #[test]
    fn test_is_nonce_transaction_detects_advance_nonce() {
        let signer = test_signer_pubkey();
        let nonce_account =
            Pubkey::from_str("9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz").unwrap();
        let usdc_mint = Pubkey::from_str(TEST_USDC_MINT).unwrap();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();

        let msg_bytes =
            build_nonce_tx_message(&signer, &nonce_account, &recipient, &usdc_mint, 5000, 6);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let result = SolanaVerifier::is_nonce_transaction(&parsed);
        assert!(
            result.is_some(),
            "transaction with AdvanceNonceAccount first should be detected as nonce tx"
        );
        assert_eq!(
            result.unwrap(),
            nonce_account,
            "should return the nonce account pubkey"
        );
    }

    #[test]
    fn test_is_nonce_transaction_returns_none_for_normal_tx() {
        // A normal SPL transfer has no AdvanceNonceAccount instruction
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let msg_bytes = build_spl_transfer_message(&signer, &recipient, 5000);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let result = SolanaVerifier::is_nonce_transaction(&parsed);
        assert!(
            result.is_none(),
            "normal SPL transfer must not be detected as nonce tx, got: {:?}",
            result
        );
    }

    #[test]
    fn test_is_nonce_transaction_wrong_discriminator() {
        // Build a SystemProgram instruction with wrong data (not AdvanceNonceAccount)
        let signer = test_signer_pubkey();
        let system = Pubkey::SYSTEM_PROGRAM;

        // Discriminator [0, 0, 0, 0] = CreateAccount (not AdvanceNonce)
        let create_account_data: &[u8] =
            &[0, 0, 0, 0, 0x40, 0x42, 0x0F, 0x00, 0x00, 0x00, 0x00, 0x00];
        let account_keys = [signer, system];
        let msg_bytes = build_legacy_message_raw(
            &account_keys,
            &[(1, &[0, 1], create_account_data)], // SystemProgram at idx 1
        );
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let result = SolanaVerifier::is_nonce_transaction(&parsed);
        assert!(
            result.is_none(),
            "wrong system program discriminator must not be detected as nonce tx"
        );
    }

    #[test]
    fn test_is_nonce_transaction_first_ix_not_system_program() {
        // When the first instruction is NOT the system program, it's not a nonce tx
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let usdc_mint = Pubkey::from_str(TEST_USDC_MINT).unwrap();
        // TransferChecked first (no AdvanceNonce)
        let msg_bytes =
            build_spl_transfer_checked_message(&signer, &recipient, &usdc_mint, 5000, 6);
        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();

        let result = SolanaVerifier::is_nonce_transaction(&parsed);
        assert!(
            result.is_none(),
            "TransferChecked first must not be nonce tx"
        );
    }

    #[test]
    fn test_full_transfer_checked_extraction_via_transaction() {
        let signer = test_signer_pubkey();
        let recipient = Pubkey::from_str(TEST_RECIPIENT).unwrap();
        let usdc_mint = Pubkey::from_str(TEST_USDC_MINT).unwrap();
        let msg_bytes =
            build_spl_transfer_checked_message(&signer, &recipient, &usdc_mint, 25000, 6);
        let tx_bytes = wrap_in_transaction(&msg_bytes);
        let base64_tx = encode_base64(&tx_bytes);

        let verifier = test_verifier();
        let tx = verifier
            .decode_and_validate_transaction(&base64_tx)
            .unwrap();
        let message = tx.parse_message().unwrap();
        let transfer = extract_spl_transfer(&message).unwrap();

        assert_eq!(transfer.destination, recipient);
        assert_eq!(transfer.amount, 25000);
        assert_eq!(transfer.mint, Some(usdc_mint));
    }
}
