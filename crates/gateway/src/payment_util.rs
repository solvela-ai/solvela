//! Shared payment utility functions for extracting wallet addresses from
//! x402 payment payloads and Solana transactions.

use base64::Engine;
use tracing::warn;

use solvela_x402::solana_types::VersionedTransaction;

/// Extract the payer wallet address from a payment payload.
///
/// For escrow payments, uses the `agent_pubkey` field (the depositor).
/// For direct payments, decodes the base64 transaction and extracts the
/// first account key (the fee payer / signer in Solana transactions).
/// Returns "unknown" if extraction fails.
pub fn extract_payer_wallet(payload: &solvela_x402::types::PaymentPayload) -> String {
    match &payload.payload {
        solvela_x402::types::PayloadData::Escrow(p) => p.agent_pubkey.clone(),
        solvela_x402::types::PayloadData::Direct(p) => {
            // Decode base64 transaction and extract first signer (fee payer)
            extract_signer_from_base64_tx(&p.transaction).unwrap_or_else(|| "unknown".to_string())
        }
    }
}

/// Attempt to extract the first signer (fee payer) public key from a
/// base64-encoded Solana versioned transaction.
pub fn extract_signer_from_base64_tx(b64_tx: &str) -> Option<String> {
    let tx_bytes = base64::engine::general_purpose::STANDARD
        .decode(b64_tx)
        .map_err(|e| warn!(error = %e, "failed to base64-decode transaction"))
        .ok()?;
    let tx = VersionedTransaction::from_bytes(&tx_bytes)
        .map_err(|e| warn!(error = %e, "failed to deserialize VersionedTransaction"))
        .ok()?;
    let msg = tx
        .parse_message()
        .map_err(|e| warn!(error = %e, "failed to parse transaction message"))
        .ok()?;
    msg.account_keys.first().map(|pk| pk.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solvela_x402::types::{SOLANA_NETWORK, USDC_MINT};

    #[test]
    fn test_extract_payer_wallet_escrow() {
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 1,
            resource: solvela_x402::types::Resource {
                url: "/test".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "escrow".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: USDC_MINT.to_string(),
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
        assert_eq!(
            extract_payer_wallet(&payload),
            "9noXzpXnkyEcKF3AeXqUHTdR59V5uvrRBUo9bwsHaByz"
        );
    }

    #[test]
    fn test_extract_payer_wallet_direct_invalid_tx() {
        let payload = solvela_x402::types::PaymentPayload {
            x402_version: 1,
            resource: solvela_x402::types::Resource {
                url: "/test".to_string(),
                method: "POST".to_string(),
            },
            accepted: solvela_x402::types::PaymentAccept {
                scheme: "exact".to_string(),
                network: SOLANA_NETWORK.to_string(),
                amount: "1000".to_string(),
                asset: USDC_MINT.to_string(),
                pay_to: "RecipientWallet".to_string(),
                max_timeout_seconds: 300,
                escrow_program_id: None,
            },
            payload: solvela_x402::types::PayloadData::Direct(solvela_x402::types::SolanaPayload {
                transaction: "not-valid-base64!!!".to_string(),
            }),
        };
        assert_eq!(extract_payer_wallet(&payload), "unknown");
    }

    #[test]
    fn test_extract_signer_from_base64_tx_invalid() {
        assert_eq!(extract_signer_from_base64_tx("not-base64!!!"), None);
        assert_eq!(extract_signer_from_base64_tx(""), None);
        assert_eq!(extract_signer_from_base64_tx("AAAA"), None);
    }
}
