//! Payment extraction, validation, and escrow claim logic.
//!
//! Functions for decoding payment headers, extracting payer wallets,
//! detecting durable nonce transactions, and firing escrow claims.

use std::sync::Arc;

use base64::Engine;
use tracing::warn;

use solvela_x402::solana_types::VersionedTransaction;

use crate::middleware::solvela_x402::decode_payment_header;
use crate::payment_util::extract_payer_wallet;
use crate::AppState;

/// Try to decode a `PaymentPayload` from the `PAYMENT-SIGNATURE` header.
///
/// Returns `None` if decoding fails -- this is intentional for backwards
/// compatibility with raw string headers used in tests (e.g., "fake-payment-for-testing").
///
/// Delegates to the shared `decode_payment_header` in the x402 middleware.
pub(crate) fn decode_payment_from_header(header: &str) -> Option<solvela_x402::types::PaymentPayload> {
    decode_payment_header(header).ok()
}

/// Extract wallet address and transaction signature from the payment header.
///
/// If the header is a valid `PaymentPayload`, extracts the actual payer wallet
/// and transaction signature. For escrow payments, uses `agent_pubkey`. For
/// direct payments, decodes the Solana transaction to get the first signer
/// (fee payer). Falls back to "unknown" if extraction fails.
pub(crate) fn extract_payment_info(header: &str) -> (String, Option<String>) {
    match decode_payment_from_header(header) {
        Some(payload) => {
            let wallet = extract_payer_wallet(&payload);
            let tx_sig = match &payload.payload {
                solvela_x402::types::PayloadData::Direct(p) => Some(p.transaction.clone()),
                solvela_x402::types::PayloadData::Escrow(p) => Some(p.deposit_tx.clone()),
            };
            (wallet, tx_sig)
        }
        None => ("unknown".to_string(), None),
    }
}

/// Detect whether a base64-encoded Solana transaction uses a durable nonce.
///
/// Durable nonce transactions have an `AdvanceNonceAccount` instruction as
/// the FIRST instruction. The System Program's `AdvanceNonceAccount` has
/// instruction discriminator `4` (little-endian u32 = `[4, 0, 0, 0]`).
///
/// Returns `false` if the transaction cannot be decoded (fail-safe: treat as
/// standard blockhash transaction with shorter replay TTL).
pub(crate) fn uses_durable_nonce(b64_tx: &str) -> bool {
    let tx_bytes = match base64::engine::general_purpose::STANDARD.decode(b64_tx) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let tx = match VersionedTransaction::from_bytes(&tx_bytes) {
        Ok(t) => t,
        Err(_) => return false,
    };
    let msg = match tx.parse_message() {
        Ok(m) => m,
        Err(_) => return false,
    };

    // AdvanceNonceAccount must be the first instruction
    let first_ix = match msg.instructions.first() {
        Some(ix) => ix,
        None => return false,
    };

    // Check that the program invoked is the System Program (all zeros)
    let program_key = msg.account_keys.get(first_ix.program_id_index as usize);
    let is_system_program =
        matches!(program_key, Some(pk) if *pk == solvela_x402::solana_types::Pubkey::SYSTEM_PROGRAM);

    // AdvanceNonceAccount discriminator: 4 as little-endian u32
    let is_advance_nonce = first_ix.data.len() >= 4 && first_ix.data[..4] == [4, 0, 0, 0];

    is_system_program && is_advance_nonce
}

/// Cap the claim amount against the verified deposit and client-advertised amount.
///
/// - If the verifier extracted a deposit amount, cap to that.
/// - If not (parse failure in verifier), fall back to `client_amount` as a defense-in-depth
///   upper bound to prevent over-claiming that would waste tx fees on an on-chain rejection.
fn cap_claim_amount(claim_atomic: u64, deposited: Option<u64>, client_amount: u64) -> u64 {
    match deposited {
        Some(d) => claim_atomic.min(d),
        None => claim_atomic.min(client_amount),
    }
}

/// Fire an escrow claim transaction if the payment scheme is escrow.
///
/// Prefers the durable claim queue (PostgreSQL) when a DB pool is available,
/// falling back to fire-and-forget via `claim_async` when it is not.
/// Caps the claim amount to the verified deposit to prevent over-claiming.
/// When the verifier could not extract the deposit amount, falls back to
/// `client_amount` (the gateway-advertised amount) as a defense-in-depth bound.
pub(crate) fn fire_escrow_claim(
    state: &Arc<AppState>,
    payment_scheme: &str,
    escrow_service_id: &Option<String>,
    escrow_agent_pubkey: &Option<String>,
    escrow_deposited_amount: Option<u64>,
    claim_atomic: u64,
    client_amount: u64,
) {
    if payment_scheme != "escrow" {
        return;
    }
    if let (Some(ref sid_b64), Some(ref agent_b58)) = (escrow_service_id, escrow_agent_pubkey) {
        // Cap claim amount to the verified deposit amount, falling back to client_amount
        if escrow_deposited_amount.is_none() {
            tracing::warn!(
                service_id = ?escrow_service_id,
                client_amount,
                claim_atomic,
                "escrow claim using client_amount as fallback bound (verifier did not extract deposit amount)"
            );
        }
        let claim_amount = cap_claim_amount(claim_atomic, escrow_deposited_amount, client_amount);

        // Never claim 0 -- if cost computation failed, skip the claim entirely
        if claim_amount == 0 {
            warn!(
                service_id = %sid_b64,
                agent = %agent_b58,
                "skipping escrow claim: computed claim amount is 0"
            );
            return;
        }

        if let Ok(sid) = decode_service_id(sid_b64) {
            // Prefer durable queue if DB is available
            if let Some(ref pool) = state.db_pool {
                let pool = pool.clone();
                let agent = agent_b58.clone();
                tokio::spawn(async move {
                    if let Err(e) = solvela_x402::escrow::claim_queue::enqueue_claim(
                        &pool,
                        &sid,
                        &agent,
                        claim_amount,
                        escrow_deposited_amount,
                    )
                    .await
                    {
                        tracing::warn!(error = %e, "failed to enqueue escrow claim");
                    }
                });
            } else if let Some(claimer) = &state.escrow_claimer {
                // Fallback: fire-and-forget (no DB)
                if let Ok(agent_bytes) = decode_agent_pubkey(agent_b58) {
                    claimer.claim_async(sid, agent_bytes, claim_amount);
                }
            }
        }
    }
}

/// Decode a base64-encoded `service_id` into a 32-byte array.
fn decode_service_id(b64: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| format!("invalid service_id base64: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("service_id must be 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Decode a base58-encoded agent pubkey into a 32-byte array.
fn decode_agent_pubkey(b58: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(b58)
        .into_vec()
        .map_err(|e| format!("invalid agent_pubkey base58: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "agent_pubkey must be 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // cap_claim_amount
    // =========================================================================

    #[test]
    fn test_cap_claim_amount_with_deposited() {
        assert_eq!(cap_claim_amount(2625, Some(2625), 2625), 2625);
        assert_eq!(cap_claim_amount(3000, Some(2625), 5000), 2625); // capped at deposit
        assert_eq!(cap_claim_amount(2000, Some(2625), 5000), 2000); // claim less than deposit
    }

    #[test]
    fn test_cap_claim_amount_falls_back_to_client_amount() {
        assert_eq!(cap_claim_amount(2625, None, 2625), 2625);
        assert_eq!(cap_claim_amount(3000, None, 2625), 2625); // capped at client_amount
        assert_eq!(cap_claim_amount(2000, None, 2625), 2000); // claim less than client_amount, no cap
    }

    // =========================================================================
    // decode_payment_from_header
    // =========================================================================

    #[test]
    fn test_decode_payment_from_header_valid_base64() {
        use base64::Engine;
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 2,
            resource: solvela_x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: solvela_x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: solvela_x402::types::USDC_MINT.to_string(),
                pay_to: "TestWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let result = decode_payment_from_header(&encoded);
        assert!(result.is_some());
        let decoded = result.unwrap();
        assert_eq!(decoded.x402_version, 2);
        assert_eq!(decoded.accepted.scheme, "exact");
    }

    #[test]
    fn test_decode_payment_from_header_raw_json() {
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 2,
            resource: solvela_x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: solvela_x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: solvela_x402::types::USDC_MINT.to_string(),
                pay_to: "TestWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: "dGVzdA==".to_string(),
            }),
        };
        let json_str = serde_json::to_string(&payload).unwrap();

        let result = decode_payment_from_header(&json_str);
        assert!(result.is_some());
    }

    #[test]
    fn test_decode_payment_from_header_invalid_returns_none() {
        assert!(decode_payment_from_header("garbage-data").is_none());
        assert!(decode_payment_from_header("").is_none());
        assert!(decode_payment_from_header("fake-payment-for-testing").is_none());
    }

    // =========================================================================
    // extract_payment_info
    // =========================================================================

    #[test]
    fn test_extract_payment_info_invalid_header() {
        let (wallet, tx_sig) = extract_payment_info("not-a-valid-payment");
        assert_eq!(wallet, "unknown");
        assert!(tx_sig.is_none());
    }

    #[test]
    fn test_extract_payment_info_valid_header_direct_undecodable_tx() {
        use base64::Engine;
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 2,
            resource: solvela_x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: solvela_x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: solvela_x402::types::USDC_MINT.to_string(),
                pay_to: "MyWallet123".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                // "dGVzdHR4" decodes to "testtx" -- not a valid Solana tx,
                // so payer extraction falls back to "unknown".
                transaction: "dGVzdHR4".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let (wallet, tx_sig) = extract_payment_info(&encoded);
        assert_eq!(wallet, "unknown");
        assert_eq!(tx_sig, Some("dGVzdHR4".to_string()));
    }

    #[test]
    fn test_extract_payment_info_escrow_uses_agent_pubkey() {
        use base64::Engine;
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 2,
            resource: solvela_x402::types::Resource {
                url: "/v1/chat/completions".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: solvela_x402::types::SOLANA_NETWORK.to_string(),
                amount: "2625".to_string(),
                asset: solvela_x402::types::USDC_MINT.to_string(),
                pay_to: "RecipientWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Escrow(solvela_x402::types::EscrowPayload {
                deposit_tx: "dGVzdA==".to_string(),
                service_id: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string(),
                agent_pubkey: "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz".to_string(),
            }),
        };
        let json = serde_json::to_vec(&payload).unwrap();
        let encoded = base64::engine::general_purpose::STANDARD.encode(&json);

        let (wallet, tx_sig) = extract_payment_info(&encoded);
        assert_eq!(wallet, "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz");
        assert_eq!(tx_sig, Some("dGVzdA==".to_string()));
    }

    // =========================================================================
    // decode_service_id
    // =========================================================================

    #[test]
    fn test_decode_service_id_valid() {
        use base64::Engine;
        let bytes = [42u8; 32];
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let result = decode_service_id(&b64);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), bytes);
    }

    #[test]
    fn test_decode_service_id_wrong_length() {
        use base64::Engine;
        let bytes = [42u8; 16]; // 16 bytes, not 32
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let result = decode_service_id(&b64);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("32 bytes"));
    }

    #[test]
    fn test_decode_service_id_invalid_base64() {
        let result = decode_service_id("not-valid-base64!!!");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("base64"));
    }

    // =========================================================================
    // decode_agent_pubkey
    // =========================================================================

    #[test]
    fn test_decode_agent_pubkey_valid() {
        let result = decode_agent_pubkey("11111111111111111111111111111111");
        assert!(result.is_ok());
        let bytes = result.unwrap();
        assert_eq!(bytes.len(), 32);
    }

    #[test]
    fn test_decode_agent_pubkey_wrong_length() {
        let short_bytes = [1u8; 16];
        let b58 = bs58::encode(&short_bytes).into_string();
        let result = decode_agent_pubkey(&b58);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("32 bytes"));
    }

    #[test]
    fn test_decode_agent_pubkey_invalid_base58() {
        let result = decode_agent_pubkey("00000InvalidBase58lII");
        assert!(result.is_err());
    }

    // =========================================================================
    // uses_durable_nonce
    // =========================================================================

    #[test]
    fn test_uses_durable_nonce_returns_false_for_invalid_base64() {
        assert!(!uses_durable_nonce("not-valid-base64!!!"));
    }

    #[test]
    fn test_uses_durable_nonce_returns_false_for_empty() {
        assert!(!uses_durable_nonce(""));
    }

    #[test]
    fn test_uses_durable_nonce_returns_false_for_standard_tx() {
        use solvela_x402::solana_types::Pubkey;

        let keys = vec![Pubkey::SYSTEM_PROGRAM, Pubkey::TOKEN_PROGRAM_ID];
        let msg_bytes = build_legacy_message_for_nonce_test(&keys, &[(1, &[0], &[3, 0x0C, 0x01])]);

        let mut tx_data = Vec::new();
        tx_data.push(0x01);
        tx_data.extend_from_slice(&[0xAA; 64]);
        tx_data.extend_from_slice(&msg_bytes);

        let b64 = base64::engine::general_purpose::STANDARD.encode(&tx_data);
        assert!(
            !uses_durable_nonce(&b64),
            "standard transfer should not be detected as durable nonce"
        );
    }

    #[test]
    fn test_uses_durable_nonce_returns_true_for_nonce_tx() {
        use solvela_x402::solana_types::Pubkey;

        let nonce_account = Pubkey([1u8; 32]);
        let keys = vec![
            Pubkey::SYSTEM_PROGRAM,
            nonce_account,
            Pubkey::SYSTEM_PROGRAM,
        ];
        let msg_bytes = build_legacy_message_for_nonce_test(&keys, &[(2, &[1, 0], &[4, 0, 0, 0])]);

        let mut tx_data = Vec::new();
        tx_data.push(0x01);
        tx_data.extend_from_slice(&[0xAA; 64]);
        tx_data.extend_from_slice(&msg_bytes);

        let b64 = base64::engine::general_purpose::STANDARD.encode(&tx_data);
        assert!(
            uses_durable_nonce(&b64),
            "transaction with AdvanceNonceAccount as first instruction should be detected"
        );
    }

    #[test]
    fn test_uses_durable_nonce_returns_false_when_nonce_not_first() {
        use solvela_x402::solana_types::Pubkey;

        let nonce_account = Pubkey([1u8; 32]);
        let keys = vec![
            Pubkey::SYSTEM_PROGRAM,
            nonce_account,
            Pubkey::TOKEN_PROGRAM_ID,
            Pubkey::SYSTEM_PROGRAM,
        ];
        let msg_bytes = build_legacy_message_for_nonce_test(
            &keys,
            &[(2, &[0, 1], &[3, 0x0C]), (3, &[1, 0], &[4, 0, 0, 0])],
        );

        let mut tx_data = Vec::new();
        tx_data.push(0x01);
        tx_data.extend_from_slice(&[0xAA; 64]);
        tx_data.extend_from_slice(&msg_bytes);

        let b64 = base64::engine::general_purpose::STANDARD.encode(&tx_data);
        assert!(
            !uses_durable_nonce(&b64),
            "nonce instruction not in first position should not be detected"
        );
    }

    /// Helper to build a minimal legacy message for nonce detection tests.
    fn build_legacy_message_for_nonce_test(
        account_keys: &[solvela_x402::solana_types::Pubkey],
        instructions: &[(u8, &[u8], &[u8])],
    ) -> Vec<u8> {
        let mut msg = vec![
            1,                        // num_required_signatures
            0,                        // num_readonly_signed
            1,                        // num_readonly_unsigned
            account_keys.len() as u8, // compact-u16 for small values
        ];
        for key in account_keys {
            msg.extend_from_slice(&key.0);
        }
        msg.extend_from_slice(&[0u8; 32]); // recent blockhash
        msg.push(instructions.len() as u8);
        for (pid_index, accounts, data) in instructions {
            msg.push(*pid_index);
            msg.push(accounts.len() as u8);
            msg.extend_from_slice(accounts);
            msg.push(data.len() as u8);
            msg.extend_from_slice(data);
        }
        msg
    }
}
