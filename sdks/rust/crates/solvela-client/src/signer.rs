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

// ---------------------------------------------------------------------------
// Exact-scheme golden vector (cross-SDK money-path contract)
// ---------------------------------------------------------------------------

/// Byte-exact base64 `exact`-scheme transaction for a fixed input. Pins the
/// canonical SPL `TransferChecked` (instruction discriminator 12) wire layout
/// every SDK signer must reproduce so the live gateway `exact` verifier
/// (`crates/x402/src/solana.rs`, which rejects plain `Transfer` discriminator 3)
/// accepts it. The escrow path has the sibling [`solvela_escrow_tx::deposit::GOLDEN_VECTOR_B64`];
/// this is its `exact` counterpart. The same literal is re-pinned and driven
/// through the real verifier parse/validate path in
/// `crates/x402/src/solana.rs` (`exact_golden_vector_accepted_as_usdc_transfer_checked`).
///
/// Fixed input (sibling-consistent with the escrow vector):
/// - agent keypair from seed `[42u8; 32]`
///   (pubkey `2iXtA8oeZqUU5pofxK971TCEvFGfems2AcDRaZHKD2pQ`)
/// - recipient / `pay_to` `9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM`
/// - USDC mint `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`
/// - amount `2625` atomic USDC, `decimals = 6`
/// - `recent_blockhash = [0xABu8; 32]`
/// - instruction accounts `[source_ata, mint, dest_ata, authority]` where
///   `source_ata = ATA(agent, mint)`, `dest_ata = ATA(pay_to, mint)`,
///   `authority = agent`; agent is fee payer (`account_keys[0]`) and sole signer.
///
/// DO NOT hand-edit. If this changes, the `exact` wire layout drifted —
/// recompute only after confirming the new layout is still accepted by the
/// gateway verifier (the on-chain/gateway value is ground truth, never this file).
///
/// `#[cfg(test)]`, not `pub`: this is a test-pinning artifact, not a stable
/// public API. No external crate imports it — the x402 gateway test duplicates
/// the literal (separate dependency island) and the cross-SDK drift guards read
/// it from this source file's text, so it exists only to drive this crate's own
/// parity tests. Gating it to test builds keeps it off the public/non-test
/// surface entirely (which would otherwise semver-imply a stability guarantee it
/// does not carry). The escrow sibling
/// (`solvela_escrow_tx::deposit::GOLDEN_VECTOR_B64`) is `pub` only because its
/// golden-vector test lives in a *separate* crate and must import it.
#[cfg(test)]
const EXACT_GOLDEN_VECTOR_B64: &str = "AbipfII25y2dIV7pTiOYf+qp9tAqiikKnoJqJnMsMNmGEMP1hDxqdcaeDIPxW3EJq5WUYR+V27kgDsjLvDsXDwEBAAIFGX9rI+FshTLGq8g4+s1ep4m+DHaykgM0A5v6iz02jWEUtdnlbYnE3avA84DOO4wvyfA9bjZxRumTasKUlOhjntPqjPWsrKjNBSB1EhdcQ871Sl3Znt4goWtVJTc485fcBt324ddloZPZy+FGzut5rBy0he1fWzeROoz1hX7/AKnG+nrzvtutOj1l82qryXQxsbvkwtL24OR8pgIDRS9dYaurq6urq6urq6urq6urq6urq6urq6urq6urq6urq6urAQMEAQQCAAoMQQoAAAAAAAAG";

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
/// Fetches a recent blockhash via JSON-RPC, then delegates the byte-exact
/// construction to [`build_exact_transfer_tx`].
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
    let blockhash = get_latest_blockhash(rpc_url, http).await?;
    build_exact_transfer_tx(wallet, recipient, amount_atomic, blockhash)
}

/// Build and sign a USDC-SPL `TransferChecked` (instruction discriminator 12)
/// transaction for an exact payment, against a caller-supplied `recent_blockhash`.
///
/// Pure (no I/O): for fixed inputs it always produces the same bytes, which is
/// what lets the `EXACT_GOLDEN_VECTOR_B64` test vector pin the wire layout. The agent wallet is
/// the transaction fee payer (`account_keys[0]`) and the sole signer — this is
/// exactly the structure the gateway `exact` verifier accepts (it validates the
/// first signature against `account_keys[0]` and requires `TransferChecked` so
/// the USDC mint is on-chain-verifiable; see `crates/x402/src/solana.rs` Step 5).
/// The gateway/facilitator is the SOL fee payer only at the settlement-broadcast
/// layer, not in the signed payload the agent produces here.
///
/// Instruction accounts, in canonical SPL `TransferChecked` order:
/// `[source_ata, mint, dest_ata, authority]` where
/// `source_ata = ATA(agent, USDC_MINT)` and `dest_ata = ATA(recipient, USDC_MINT)`,
/// and instruction data is `[12] || amount(u64 LE) || decimals(=6)`.
///
/// # Errors
///
/// Returns `SignerError::TransactionBuild` if the recipient address is invalid
/// or the SPL instruction cannot be built.
pub(crate) fn build_exact_transfer_tx(
    wallet: &Wallet,
    recipient: &str,
    amount_atomic: u64,
    blockhash: Hash,
) -> Result<String, SignerError> {
    // Reject a zero amount BEFORE building anything. A `0` would produce a
    // perfectly valid `$0` `TransferChecked` that the gateway accepts as a
    // signed payment, settling nothing while the request proceeds. Fail closed
    // at the boundary — parity with the Python `_sign_exact_payment` guard and
    // the escrow `DepositParams` amount check (money-path: reject zero amount).
    if amount_atomic == 0 {
        return Err(SignerError::TransactionBuild(
            "exact transfer amount must be greater than zero".to_string(),
        ));
    }

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

    const GOLDEN_PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";

    /// Deterministic agent wallet from seed `[42u8; 32]` — identical to the
    /// escrow golden vector's agent keypair, for sibling consistency.
    fn golden_agent_wallet() -> Wallet {
        use ed25519_dalek::SigningKey;
        let seed = [42u8; 32];
        let sk = SigningKey::from_bytes(&seed);
        let mut kp = [0u8; 64];
        kp[..32].copy_from_slice(&sk.to_bytes());
        kp[32..].copy_from_slice(sk.verifying_key().as_bytes());
        Wallet::from_keypair_bytes(&kp).expect("valid golden keypair")
    }

    /// Pinning test (a): the reference `exact` construction reproduces the
    /// canonical golden vector byte-for-byte. `sign_exact_payment` delegates to
    /// `build_exact_transfer_tx` after fetching the blockhash, so proving the
    /// pure builder reproduces the vector for the fixed blockhash proves
    /// `sign_exact_payment` does too (task item 4 — Rust-SDK parity).
    ///
    /// NEVER edit `EXACT_GOLDEN_VECTOR_B64` to match this output — the vector is
    /// the cross-SDK contract; if this drifts, the wire layout changed.
    #[test]
    fn exact_tx_matches_golden_vector() {
        let wallet = golden_agent_wallet();
        let blockhash = Hash::new_from_array([0xABu8; 32]);
        let b64 = build_exact_transfer_tx(&wallet, GOLDEN_PROVIDER, 2625, blockhash)
            .expect("golden exact build should succeed");
        assert_eq!(
            b64, EXACT_GOLDEN_VECTOR_B64,
            "exact-tx byte layout drifted from the pinned golden vector"
        );
    }

    /// The build is pure: identical fixed input always yields identical bytes
    /// (no nonce, no clock — the blockhash is supplied).
    #[test]
    fn exact_tx_is_deterministic() {
        let wallet = golden_agent_wallet();
        let blockhash = Hash::new_from_array([0xABu8; 32]);
        let a = build_exact_transfer_tx(&wallet, GOLDEN_PROVIDER, 2625, blockhash).unwrap();
        let b = build_exact_transfer_tx(&wallet, GOLDEN_PROVIDER, 2625, blockhash).unwrap();
        assert_eq!(a, b);
    }

    /// A zero amount must be rejected before any tx is built — parity with the
    /// Python `_sign_exact_payment` guard. A `0` would otherwise produce a valid
    /// signed `$0` `TransferChecked` (money-path: reject zero amount).
    #[test]
    fn exact_zero_amount_rejected() {
        let wallet = golden_agent_wallet();
        let blockhash = Hash::new_from_array([0xABu8; 32]);
        let err = build_exact_transfer_tx(&wallet, GOLDEN_PROVIDER, 0, blockhash)
            .expect_err("zero amount must be rejected");
        match err {
            SignerError::TransactionBuild(msg) => {
                assert!(
                    msg.contains("greater than zero"),
                    "unexpected error message: {msg}"
                );
            }
            other => panic!("expected TransactionBuild, got {other:?}"),
        }
    }

    /// A non-zero amount (e.g. the golden vector's 2625) is unaffected by the
    /// zero guard — the builder still succeeds and reproduces the vector.
    #[test]
    fn exact_nonzero_amount_still_builds() {
        let wallet = golden_agent_wallet();
        let blockhash = Hash::new_from_array([0xABu8; 32]);
        let b64 = build_exact_transfer_tx(&wallet, GOLDEN_PROVIDER, 2625, blockhash)
            .expect("non-zero amount must build");
        assert_eq!(b64, EXACT_GOLDEN_VECTOR_B64);
    }

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

    // --- FIX-5: client floor vs gateway buffer compatibility ---

    /// The gateway's `MIN_EXPIRY_BUFFER_SLOTS`, hardcoded here because the SDK
    /// cannot depend on the x402 crate. Source of truth:
    /// `crates/x402/src/escrow/verifier.rs` (`MIN_EXPIRY_BUFFER_SLOTS = 50`),
    /// which mirrors the on-chain `MIN_EXPIRY_BUFFER` in
    /// `programs/escrow/src/instructions/deposit.rs`. If the gateway value ever
    /// rises above 150, these tests must be revisited.
    const GATEWAY_MIN_EXPIRY_BUFFER_SLOTS: u64 = 50;

    #[test]
    fn client_floor_at_least_gateway_buffer() {
        // The client's floor must be >= the gateway's lower bound so a deposit
        // floored to the client minimum is never bounced for being too close.
        // Route the floor through `escrow_expiry_slot` (a 0-second timeout hits
        // the floor exactly) so the comparison is computed at runtime rather
        // than const-folded.
        let current = 1_000_000u64;
        let floored_buffer = escrow_expiry_slot(current, 0) - current;
        assert_eq!(
            floored_buffer, MIN_ESCROW_EXPIRY_SLOTS_AHEAD,
            "a 0-second timeout must produce exactly the client floor"
        );
        assert!(
            floored_buffer >= GATEWAY_MIN_EXPIRY_BUFFER_SLOTS,
            "client expiry floor ({floored_buffer}) must be >= gateway buffer ({GATEWAY_MIN_EXPIRY_BUFFER_SLOTS})"
        );
    }

    #[test]
    fn client_never_signs_below_gateway_buffer_for_any_timeout() {
        // For ANY max_timeout_seconds (including 0 and the saturating extreme),
        // the slots between the chosen expiry and the current slot must be at
        // least the gateway's lower bound. The verifier enforces only this
        // LOWER bound — there is NO upper bound in
        // `crates/x402/src/escrow/verifier.rs` (verified), so the client's
        // cap (MAX_ESCROW_EXPIRY_SLOTS_AHEAD = 10_000) is always accepted and
        // the floor-up for short timeouts is known-safe.
        let current = 1_000_000u64;
        for t in [
            0u64,
            1,
            10,
            19, // 19 s × 2.5 = 47.5 → 47 slots, below the gateway buffer pre-floor
            20,
            59,
            60,
            300,
            3600,
            u64::MAX,
        ] {
            let expiry = escrow_expiry_slot(current, t);
            let buffer = expiry - current;
            assert!(
                buffer >= GATEWAY_MIN_EXPIRY_BUFFER_SLOTS,
                "timeout {t}s produced buffer {buffer} < gateway minimum {GATEWAY_MIN_EXPIRY_BUFFER_SLOTS}"
            );
        }
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
