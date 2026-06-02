//! Byte-exact golden vector for the escrow deposit transaction.
//!
//! This pins the on-chain-verified deposit-tx wire layout against a fixed
//! input so any drift in the discriminator, account ordering, instruction data,
//! expiry math, or ed25519 signing fails CI. The identical vector is asserted
//! in `solvela-x402` (`deposit_tx_golden_vector`) so both crates agree.

use solvela_escrow_tx::deposit::{build_deposit_tx, DepositParams, GOLDEN_VECTOR_B64};

const PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

/// Build the deterministic 64-byte agent keypair from seed `[42u8; 32]`.
fn golden_keypair_b58() -> String {
    use ed25519_dalek::SigningKey;
    let seed = [42u8; 32];
    let signing_key = SigningKey::from_bytes(&seed);
    let mut keypair_bytes = [0u8; 64];
    keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
    keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
    bs58::encode(&keypair_bytes).into_string()
}

fn golden_params() -> DepositParams {
    DepositParams {
        agent_keypair_b58: zeroize::Zeroizing::new(golden_keypair_b58()),
        provider_wallet_b58: PROVIDER.to_string(),
        usdc_mint_b58: USDC_MINT.to_string(),
        escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
        amount: 2625,
        service_id: [7u8; 32],
        expiry_slot: 1_000_750,
        recent_blockhash: [0xABu8; 32],
    }
}

#[test]
fn deposit_tx_matches_golden_vector() {
    let b64 = build_deposit_tx(&golden_params()).expect("golden build should succeed");
    assert_eq!(
        b64, GOLDEN_VECTOR_B64,
        "deposit-tx byte layout drifted from the pinned golden vector"
    );
}

/// Deterministic: same fixed input always yields the same bytes (the build is
/// pure — no nonce, no clock).
#[test]
fn deposit_tx_is_deterministic() {
    let a = build_deposit_tx(&golden_params()).unwrap();
    let b = build_deposit_tx(&golden_params()).unwrap();
    assert_eq!(a, b);
}
