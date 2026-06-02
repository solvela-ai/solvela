use base64::Engine;
use solana_sdk::hash::Hash;
use solana_sdk::message::Message;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::transaction::Transaction;

use solvela_protocol::{
    EscrowPayload, PayloadData, PaymentAccept, PaymentPayload, Resource, SolanaPayload, USDC_MINT,
    X402_VERSION,
};

use crate::error::SignerError;
use crate::wallet::Wallet;

const USDC_DECIMALS: u8 = 6;

/// Hard upper bound on how many Solana slots into the future an escrow's expiry
/// may be set. Mirrors the CLI's `MAX_ESCROW_EXPIRY_SLOTS_AHEAD`
/// (`crates/cli/src/commands/util.rs`). Solana slots are ~400 ms, so `10_000`
/// slots ≈ 66 minutes — above any legitimate `max_timeout_seconds` while still
/// rejecting a malicious/misconfigured gateway from pushing expiry to never.
const MAX_ESCROW_EXPIRY_SLOTS_AHEAD: u64 = 10_000;

/// Minimum number of Solana slots into the future an escrow's expiry must be
/// set. This mirrors AND exceeds the gateway's `MIN_EXPIRY_BUFFER_SLOTS = 50`:
/// the gateway rejects a deposit whose `expiry_slot` is fewer than 50 slots
/// ahead of the current slot, so a too-near expiry would have the deposit
/// bounced. The extra headroom (150 vs. 50) absorbs network latency and
/// slot-skew between the slot we read here and the slot the gateway observes
/// when it verifies, preventing a borderline-but-valid expiry from being
/// rejected. ~150 slots ≈ 60 s.
const MIN_ESCROW_EXPIRY_SLOTS_AHEAD: u64 = 150;

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
/// # Errors
///
/// Returns `SignerError::TransactionBuild` if `PaymentPayload` cannot be
/// serialized to JSON (should never occur since all fields implement
/// `Serialize`, but we propagate rather than panic in library code).
pub(crate) fn encode_payment_header(payload: &PaymentPayload) -> Result<String, SignerError> {
    let json = serde_json::to_string(payload)
        .map_err(|e| SignerError::TransactionBuild(format!("payload serialize: {e}")))?;
    Ok(base64::engine::general_purpose::STANDARD.encode(json.as_bytes()))
}

// ---------------------------------------------------------------------------
// Escrow signing
// ---------------------------------------------------------------------------

/// Fetch the current slot from a Solana RPC endpoint via JSON-RPC.
///
/// Mirrors the CLI's `fetch_current_slot` (`crates/cli/src/commands/solana_tx.rs`)
/// and the `get_latest_blockhash` style above — uses `getSlot` with commitment
/// `confirmed`.
///
/// # Errors
///
/// Returns `SignerError::RpcError` if the request fails, the HTTP status is not
/// success, the body is unparseable, the RPC returns an error object, or the
/// `result` field is missing.
pub(crate) async fn get_current_slot(
    rpc_url: &str,
    http: &reqwest::Client,
) -> Result<u64, SignerError> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": [{"commitment": "confirmed"}]
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
        .map_err(|e| SignerError::RpcError(format!("failed to parse getSlot response: {e}")))?;

    if let Some(err_obj) = json.get("error") {
        return Err(SignerError::RpcError(format!("RPC error: {err_obj}")));
    }

    json["result"]
        .as_u64()
        .ok_or_else(|| SignerError::RpcError("getSlot response missing result field".to_string()))
}

/// Compute the Solana slot at which an escrow PDA created now should expire.
///
/// Saturating arithmetic, then clamped into
/// `[MIN_ESCROW_EXPIRY_SLOTS_AHEAD, MAX_ESCROW_EXPIRY_SLOTS_AHEAD]`. The
/// saturation closes the wrap-to-zero case (instantly-refundable PDA →
/// reverse-payment race) and the cap closes the unbounded-future case.
///
/// The minimum floor (`MIN_ESCROW_EXPIRY_SLOTS_AHEAD`) mirrors and exceeds the
/// gateway's `MIN_EXPIRY_BUFFER_SLOTS = 50`: a too-near expiry would otherwise
/// be bounced by the gateway, and the extra headroom absorbs network/slot-skew
/// between the slot we read and the slot the gateway verifies against. Note:
/// the CLI's `escrow_expiry_slot` (`crates/cli/src/commands/util.rs`) currently
/// lacks this floor and should be brought in line separately.
fn escrow_expiry_slot(current_slot: u64, max_timeout_seconds: u64) -> u64 {
    let timeout_slots = max_timeout_seconds.saturating_mul(1000).saturating_div(400);
    let effective =
        timeout_slots.clamp(MIN_ESCROW_EXPIRY_SLOTS_AHEAD, MAX_ESCROW_EXPIRY_SLOTS_AHEAD);
    current_slot.saturating_add(effective)
}

/// Generate a unique 32-byte `service_id`.
///
/// Issue #118 security invariant (see `crates/x402/src/escrow/AGENTS.md`): the
/// `service_id` MUST mix a per-request CSPRNG nonce so two identical requests
/// never share a `service_id` → escrow PDA → vault ATA, which would be
/// off-chain-derivable before broadcast (front-running, confidentiality leak).
/// The signer does not have the request body at this layer, so we hash 32
/// CSPRNG bytes directly — the nonce is the entire input, which satisfies the
/// hard invariant (distinct per call). Mirrors the CLI's `generate_service_id`
/// `SHA-256(body || nonce)` shape as closely as the available inputs allow.
fn generate_service_id() -> Result<[u8; 32], SignerError> {
    use sha2::{Digest, Sha256};
    let mut nonce = [0u8; 32];
    getrandom::getrandom(&mut nonce)
        .map_err(|e| SignerError::TransactionBuild(format!("getrandom failed: {e}")))?;
    let mut hasher = Sha256::new();
    hasher.update(nonce);
    let hash = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash);
    Ok(id)
}

/// Build and sign an escrow `deposit` transaction for an escrow-scheme payment.
///
/// Concurrently fetches the current slot and a recent blockhash, derives a
/// fresh CSPRNG-backed `service_id`, computes the expiry slot via the
/// CLI-identical formula, and delegates the byte-exact transaction construction
/// to the shared, on-chain-verified `solvela_escrow_tx::deposit::build_deposit_tx`.
///
/// Returns `(deposit_tx_b64, service_id)` so the caller can build the payload.
///
/// `amount_atomic` is the already-parsed, already-budget-validated payment
/// amount supplied by the caller (mirrors `sign_exact_payment`), so the amount
/// is parsed exactly once on the request path.
///
/// # Errors
///
/// Returns `SignerError::RpcError` if slot/blockhash fetch fails,
/// `SignerError::TransactionBuild` if the escrow program ID is missing, the
/// `pay_to` address is not a valid Solana pubkey, the CSPRNG fails, or the
/// deposit-tx build fails.
pub(crate) async fn sign_escrow_payment(
    wallet: &Wallet,
    rpc_url: &str,
    http: &reqwest::Client,
    accept: &PaymentAccept,
    amount_atomic: u64,
) -> Result<(String, [u8; 32]), SignerError> {
    let escrow_program_id = accept.escrow_program_id.as_deref().ok_or_else(|| {
        SignerError::TransactionBuild("escrow scheme missing program ID".to_string())
    })?;

    // FIX-7: fail fast — validate `pay_to` parses as a Solana pubkey before any
    // RPC calls or CSPRNG work, so a malformed recipient is rejected cheaply.
    let _provider_pubkey: Pubkey = accept.pay_to.parse().map_err(|e| {
        SignerError::TransactionBuild(format!("invalid escrow pay_to address: {e}"))
    })?;

    let service_id = generate_service_id()?;

    // Fetch current slot and recent blockhash concurrently — both are needed
    // before the deposit tx can be built, and they are independent.
    let (slot_res, blockhash_res) = tokio::join!(
        get_current_slot(rpc_url, http),
        get_latest_blockhash(rpc_url, http),
    );
    let current_slot = slot_res?;
    let blockhash = blockhash_res?;
    let recent_blockhash: [u8; 32] = blockhash.to_bytes();

    let expiry_slot = escrow_expiry_slot(current_slot, accept.max_timeout_seconds);

    let deposit_tx =
        solvela_escrow_tx::deposit::build_deposit_tx(&solvela_escrow_tx::deposit::DepositParams {
            agent_keypair_b58: wallet.to_keypair_b58(),
            provider_wallet_b58: accept.pay_to.clone(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: escrow_program_id.to_string(),
            amount: amount_atomic,
            service_id,
            expiry_slot,
            recent_blockhash,
        })
        .map_err(|e| {
            SignerError::TransactionBuild(format!("escrow deposit tx build failed: {e}"))
        })?;

    Ok((deposit_tx, service_id))
}

/// Build the escrow `PaymentPayload` for the PAYMENT-SIGNATURE header.
#[must_use]
pub(crate) fn build_escrow_payment_payload(
    resource: &Resource,
    accept: &PaymentAccept,
    deposit_tx_b64: &str,
    service_id: &[u8; 32],
    agent_pubkey: &str,
) -> PaymentPayload {
    PaymentPayload {
        x402_version: X402_VERSION,
        resource: resource.clone(),
        accepted: accept.clone(),
        payload: PayloadData::Escrow(EscrowPayload {
            deposit_tx: deposit_tx_b64.to_string(),
            service_id: base64::engine::general_purpose::STANDARD.encode(service_id),
            agent_pubkey: agent_pubkey.to_string(),
        }),
    }
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

    // --- escrow expiry math (mirrors CLI escrow_expiry_slot) ---

    #[test]
    fn escrow_expiry_typical_300s() {
        // 300 s × 2.5 slots/s = 750 slots ahead. NO min floor.
        assert_eq!(escrow_expiry_slot(1_000_000, 300), 1_000_750);
    }

    #[test]
    fn escrow_expiry_clamps_huge_values() {
        assert_eq!(
            escrow_expiry_slot(1_000_000, u64::MAX),
            1_000_000 + MAX_ESCROW_EXPIRY_SLOTS_AHEAD,
        );
    }

    #[test]
    fn escrow_expiry_zero_seconds_applies_min_floor() {
        // FIX-2: a too-small timeout is floored to MIN_ESCROW_EXPIRY_SLOTS_AHEAD
        // so the gateway (MIN_EXPIRY_BUFFER_SLOTS = 50) does not bounce it.
        assert_eq!(
            escrow_expiry_slot(1_000_000, 0),
            1_000_000 + MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
        );
    }

    #[test]
    fn escrow_expiry_typical_300s_unaffected_by_floor() {
        // 300 s → 750 slots is between the floor (150) and cap (10_000), so the
        // normal case is unchanged.
        assert_eq!(escrow_expiry_slot(1_000_000, 300), 1_000_750);
    }

    #[test]
    fn escrow_expiry_saturates_current_slot_overflow() {
        assert_eq!(escrow_expiry_slot(u64::MAX - 100, 300), u64::MAX);
    }

    // --- service_id CSPRNG invariant (#118) ---

    #[test]
    fn identical_input_distinct_service_ids() {
        // The signer takes no body, so the only input is the CSPRNG nonce.
        // Two calls must never collide.
        let a = generate_service_id().unwrap();
        let b = generate_service_id().unwrap();
        assert_ne!(a, b, "service_id must mix a per-call CSPRNG nonce (#118)");
        assert_eq!(a.len(), 32);
    }

    // --- escrow payload shape ---

    #[test]
    fn build_escrow_payment_payload_shape() {
        let program_id = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";
        let accept = PaymentAccept {
            scheme: "escrow".to_string(),
            network: solvela_protocol::SOLANA_NETWORK.to_string(),
            amount: "2625".to_string(),
            asset: USDC_MINT.to_string(),
            pay_to: Pubkey::new_unique().to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some(program_id.to_string()),
        };
        let resource = Resource {
            url: "/v1/chat/completions".to_string(),
            method: "POST".to_string(),
        };
        let service_id = [7u8; 32];
        let agent = Pubkey::new_unique().to_string();

        let payload =
            build_escrow_payment_payload(&resource, &accept, "ZGVwb3NpdA==", &service_id, &agent);

        assert_eq!(payload.x402_version, X402_VERSION);
        assert_eq!(payload.accepted.scheme, "escrow");
        match &payload.payload {
            PayloadData::Escrow(p) => {
                assert_eq!(p.deposit_tx, "ZGVwb3NpdA==");
                assert_eq!(p.agent_pubkey, agent);
                // service_id is standard-base64 of the 32 bytes.
                assert_eq!(
                    p.service_id,
                    base64::engine::general_purpose::STANDARD.encode(service_id)
                );
            }
            PayloadData::Direct(_) => panic!("expected Escrow variant"),
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
        let encoded = encode_payment_header(&payload).unwrap();

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
