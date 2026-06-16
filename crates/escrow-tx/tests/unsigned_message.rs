//! Golden-vector parity for the pubkey-only `build_deposit_message` path.
//!
//! The unsigned-message builder must reproduce the EXACT message bytes embedded
//! in the signed `GOLDEN_VECTOR_B64`, so that an external signer can assemble
//! `compact-u16(1) || signer_sig(64) || message` and get a transaction
//! byte-identical to what `build_deposit_tx` produces today.
//!
//! Wire layout of the signed golden vector:
//!   `compact-u16(1) [1 byte] || ed25519_sig [64 bytes] || message`
//! So the message slice is `bytes[65..]`.
//!
//! These vectors share the SAME fixed inputs as `golden_vector.rs` and
//! `deposit.rs` (agent pubkey derived from seed `[42u8; 32]`, provider
//! `9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM`, USDC mint
//! `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v`, program
//! `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`, `service_id=[7u8; 32]`,
//! `expiry_slot=1_000_750`, `recent_blockhash=[0xABu8; 32]`, `amount=2625`).
//!
//! NEVER edit `GOLDEN_VECTOR_B64` to match this output — the canonical/on-chain
//! signed value is ground truth. If the message slice does not match, the layout
//! drifted; STOP.

use base64::Engine as _;

use solvela_escrow_tx::deposit::{
    build_deposit_message, derive_deposit_addresses, DepositError, UnsignedDepositParams,
    GOLDEN_VECTOR_B64,
};

const PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

/// Derive the 32-byte agent pubkey from the fixed golden seed `[42u8; 32]`.
fn golden_agent_pubkey() -> [u8; 32] {
    use ed25519_dalek::SigningKey;
    let signing_key = SigningKey::from_bytes(&[42u8; 32]);
    signing_key.verifying_key().to_bytes()
}

fn golden_unsigned_params() -> UnsignedDepositParams {
    UnsignedDepositParams {
        agent_pubkey: golden_agent_pubkey(),
        provider_wallet_b58: PROVIDER.to_string(),
        usdc_mint_b58: USDC_MINT.to_string(),
        escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
        amount: 2625,
        service_id: [7u8; 32],
        expiry_slot: 1_000_750,
        recent_blockhash: [0xABu8; 32],
    }
}

/// The message slice of the signed golden vector: drop the 1-byte signature
/// count prefix and the 64-byte ed25519 signature.
fn golden_message_slice() -> Vec<u8> {
    let signed = base64::engine::general_purpose::STANDARD
        .decode(GOLDEN_VECTOR_B64)
        .expect("golden vector must be valid base64");
    assert!(
        signed.len() > 65,
        "signed golden vector must be longer than the 65-byte sig prefix"
    );
    // Sanity: compact-u16(1) signature count.
    assert_eq!(
        signed[0], 0x01,
        "golden vector must encode exactly 1 signature"
    );
    signed[65..].to_vec()
}

/// PARITY CONTRACT: the pubkey-only message builder reproduces the message
/// portion of the signed golden vector byte-for-byte.
#[test]
fn build_deposit_message_matches_golden_message_slice() {
    let msg = build_deposit_message(&golden_unsigned_params()).expect("golden message must build");
    let expected = golden_message_slice();
    assert_eq!(
        msg, expected,
        "unsigned message bytes drifted from the golden vector's message slice"
    );
}

/// Re-assembling `compact-u16(1) || sig(64) || message` from the unsigned
/// message and the golden vector's own signature must reproduce the full signed
/// golden vector. This proves an external signer can sign `message` and rebuild
/// the exact canonical transaction.
#[test]
fn reassembling_unsigned_message_with_golden_signature_reproduces_full_tx() {
    let signed = base64::engine::general_purpose::STANDARD
        .decode(GOLDEN_VECTOR_B64)
        .expect("valid base64");
    let signature = &signed[1..65]; // the canonical signature from the golden vector

    let msg = build_deposit_message(&golden_unsigned_params()).expect("message must build");

    let mut reassembled = Vec::with_capacity(1 + 64 + msg.len());
    reassembled.push(0x01);
    reassembled.extend_from_slice(signature);
    reassembled.extend_from_slice(&msg);

    let reassembled_b64 = base64::engine::general_purpose::STANDARD.encode(&reassembled);
    assert_eq!(
        reassembled_b64, GOLDEN_VECTOR_B64,
        "compact-u16(1) || golden_sig || message must reproduce the signed golden vector"
    );
}

/// The unsigned builder is pure: same fixed input always yields the same bytes.
#[test]
fn build_deposit_message_is_deterministic() {
    let a = build_deposit_message(&golden_unsigned_params()).unwrap();
    let b = build_deposit_message(&golden_unsigned_params()).unwrap();
    assert_eq!(a, b);
}

/// Zero amount must be rejected before any message is built (money-path: reject
/// zero amount).
#[test]
fn build_deposit_message_rejects_zero_amount() {
    let mut params = golden_unsigned_params();
    params.amount = 0;
    let err = build_deposit_message(&params).expect_err("zero amount must be rejected");
    assert!(
        matches!(err, DepositError::ZeroAmount),
        "expected ZeroAmount, got {err:?}"
    );
}

/// A malformed provider address fails closed (no silent fallback to a default
/// address).
#[test]
fn build_deposit_message_rejects_bad_provider() {
    let mut params = golden_unsigned_params();
    params.provider_wallet_b58 = "not-base58-!!!".to_string();
    let err = build_deposit_message(&params).expect_err("bad provider must be rejected");
    assert!(
        matches!(
            err,
            DepositError::InvalidAddress {
                field: "provider_wallet",
                ..
            }
        ),
        "expected InvalidAddress(provider_wallet), got {err:?}"
    );
}

/// `derive_deposit_addresses` must produce the same escrow PDA + vault ATA that
/// the message builder consumes — the gateway uses this to populate the
/// `decoded_intent` so a client can re-derive and verify before signing.
#[test]
fn derive_deposit_addresses_matches_canonical_derivation() {
    use solvela_escrow_tx::pda::{decode_bs58_pubkey, derive_ata_address, find_program_address};

    let agent = golden_agent_pubkey();
    let service_id = [7u8; 32];

    let derived = derive_deposit_addresses(&agent, &service_id, USDC_MINT, ESCROW_PROGRAM)
        .expect("derivation must succeed");

    let program = decode_bs58_pubkey(ESCROW_PROGRAM).unwrap();
    let mint = decode_bs58_pubkey(USDC_MINT).unwrap();
    let (expected_pda, _) =
        find_program_address(&[b"escrow", &agent, &service_id], &program).unwrap();
    let expected_vault = derive_ata_address(&expected_pda, &mint).unwrap();

    assert_eq!(derived.escrow_pda, expected_pda, "escrow PDA mismatch");
    assert_eq!(derived.vault_ata, expected_vault, "vault ATA mismatch");
}
