use base64::Engine;
use solana_sdk::hash::Hash;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::Transaction;

use solvela_protocol::{
    PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, USDC_MINT, X402_VERSION,
};

use crate::error::SignerError;
use crate::wallet::Wallet;

const USDC_DECIMALS: u8 = 6;

/// Associated Token Program ID (well-known constant).
const ASSOCIATED_TOKEN_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";

/// Compute the associated token address for a wallet and mint.
pub(crate) fn associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    // SAFETY: ASSOCIATED_TOKEN_PROGRAM_ID is a compile-time constant string;
    // its base58 form is verified valid by the SPL spec and exercised by tests.
    let ata_program: Pubkey = ASSOCIATED_TOKEN_PROGRAM_ID
        .parse()
        .expect("ATA program ID is valid");
    let (ata, _bump) = Pubkey::find_program_address(
        &[wallet.as_ref(), spl_token::id().as_ref(), mint.as_ref()],
        &ata_program,
    );
    ata
}

/// Fetch the latest blockhash from a Solana RPC endpoint via JSON-RPC.
async fn get_latest_blockhash(rpc_url: &str, http: &reqwest::Client) -> Result<Hash, SignerError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getLatestBlockhash",
        "params": []
    });

    let resp = http
        .post(rpc_url)
        .json(&body)
        .send()
        .await
        .map_err(|e| SignerError::RpcError(e.to_string()))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(SignerError::RpcError(format!("RPC returned HTTP {status}")));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| SignerError::RpcError(format!("failed to parse RPC response: {e}")))?;

    if let Some(err_obj) = json.get("error") {
        return Err(SignerError::RpcError(format!("RPC error: {err_obj}")));
    }

    let blockhash_str = json["result"]["value"]["blockhash"]
        .as_str()
        .ok_or_else(|| SignerError::RpcError("missing blockhash in RPC response".to_string()))?;

    blockhash_str
        .parse()
        .map_err(|e| SignerError::RpcError(format!("invalid blockhash: {e}")))
}

/// Build and sign a USDC-SPL `TransferChecked` transaction for an exact payment.
///
/// Fetches a recent blockhash via JSON-RPC, builds the SPL Token instruction,
/// signs with the wallet's keypair, and returns a base64-encoded transaction.
///
/// # Errors
///
/// Returns `SignerError::TransactionBuild` if the recipient address is invalid
/// or the SPL instruction cannot be built, and `SignerError::RpcError` if the
/// blockhash fetch fails.
pub(crate) async fn sign_exact_payment(
    wallet: &Wallet,
    rpc_url: &str,
    http: &reqwest::Client,
    recipient: &str,
    amount_atomic: u64,
) -> Result<String, SignerError> {
    let mint: Pubkey = USDC_MINT
        .parse()
        .map_err(|e| SignerError::TransactionBuild(format!("invalid USDC mint: {e}")))?;

    let recipient_pubkey: Pubkey = recipient
        .parse()
        .map_err(|e| SignerError::TransactionBuild(format!("invalid recipient: {e}")))?;

    let source_ata = associated_token_address(&wallet.pubkey(), &mint);
    let dest_ata = associated_token_address(&recipient_pubkey, &mint);

    let ix = spl_token::instruction::transfer_checked(
        &spl_token::id(),
        &source_ata,
        &mint,
        &dest_ata,
        &wallet.pubkey(),
        &[],
        amount_atomic,
        USDC_DECIMALS,
    )
    .map_err(|e| SignerError::TransactionBuild(format!("instruction build: {e}")))?;

    let blockhash = get_latest_blockhash(rpc_url, http).await?;

    let message = Message::new(&[ix], Some(&wallet.pubkey()));
    // Bind the reconstructed `Keypair` to a local so the borrow lasts
    // for the `Transaction::new` call.
    let signing_kp = wallet.keypair();
    let tx = Transaction::new(&[&signing_kp], message, blockhash);

    let tx_bytes = bincode::serialize(&tx)
        .map_err(|e| SignerError::TransactionBuild(format!("serialize: {e}")))?;

    Ok(base64::engine::general_purpose::STANDARD.encode(tx_bytes))
}

/// Build the `PaymentPayload` struct for the PAYMENT-SIGNATURE header.
#[must_use]
pub(crate) fn build_payment_payload(
    resource: &Resource,
    accept: &PaymentAccept,
    signed_tx_b64: &str,
) -> PaymentPayload {
    PaymentPayload {
        x402_version: X402_VERSION,
        resource: resource.clone(),
        accepted: accept.clone(),
        payload: PayloadData::Direct(SolanaPayload {
            transaction: signed_tx_b64.to_string(),
        }),
    }
}

/// Encode a `PaymentPayload` as a base64 JSON string for the PAYMENT-SIGNATURE header.
///
/// # Panics
///
/// Panics if `PaymentPayload` cannot be serialized to JSON, which should
/// never occur since all fields implement `Serialize`.
#[must_use]
pub(crate) fn encode_payment_header(payload: &PaymentPayload) -> String {
    let json = serde_json::to_string(payload).expect("PaymentPayload should always serialize");
    base64::engine::general_purpose::STANDARD.encode(json.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signer::keypair::Keypair;
    use solana_sdk::signer::Signer;

    #[test]
    fn test_associated_token_address_deterministic() {
        let wallet = Pubkey::new_unique();
        let mint: Pubkey = USDC_MINT.parse().unwrap();
        let ata1 = associated_token_address(&wallet, &mint);
        let ata2 = associated_token_address(&wallet, &mint);
        assert_eq!(ata1, ata2);
        // ATA should differ from the wallet itself
        assert_ne!(ata1, wallet);
    }

    #[test]
    fn test_build_transfer_checked_instruction() {
        let payer = Keypair::new();
        let recipient = Pubkey::new_unique();
        let mint: Pubkey = USDC_MINT.parse().unwrap();
        let amount: u64 = 2_625;

        let source_ata = associated_token_address(&payer.pubkey(), &mint);
        let dest_ata = associated_token_address(&recipient, &mint);

        let ix = spl_token::instruction::transfer_checked(
            &spl_token::id(),
            &source_ata,
            &mint,
            &dest_ata,
            &payer.pubkey(),
            &[],
            amount,
            USDC_DECIMALS,
        )
        .unwrap();

        assert_eq!(ix.accounts.len(), 4);
        assert_eq!(ix.program_id, spl_token::id());
    }

    #[test]
    fn test_build_payment_payload() {
        let accept = solvela_protocol::PaymentAccept {
            scheme: "exact".to_string(),
            network: solvela_protocol::SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: Pubkey::new_unique().to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        };

        let resource = solvela_protocol::Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        };

        let payload = build_payment_payload(&resource, &accept, "dGVzdA==");
        assert_eq!(payload.x402_version, X402_VERSION);
        assert_eq!(payload.accepted.scheme, "exact");

        match &payload.payload {
            PayloadData::Direct(p) => assert_eq!(p.transaction, "dGVzdA=="),
            PayloadData::Escrow(_) => panic!("expected Direct variant"),
        }
    }

    #[test]
    fn test_encode_payment_header_roundtrip() {
        let accept = solvela_protocol::PaymentAccept {
            scheme: "exact".to_string(),
            network: solvela_protocol::SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: Pubkey::new_unique().to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        };

        let resource = solvela_protocol::Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        };

        let payload = build_payment_payload(&resource, &accept, "dGVzdA==");
        let encoded = encode_payment_header(&payload);

        // Decode and verify roundtrip
        let decoded_bytes = base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .unwrap();
        let parsed: solvela_protocol::PaymentPayload =
            serde_json::from_slice(&decoded_bytes).unwrap();
        assert_eq!(parsed.accepted.scheme, "exact");
        assert_eq!(parsed.x402_version, X402_VERSION);
    }
}
