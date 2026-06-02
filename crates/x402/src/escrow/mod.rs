//! EscrowVerifier — verifies on-chain escrow deposits and fires claim transactions.
//!
//! The EscrowVerifier handles scheme="escrow" payments where agents deposit
//! to a PDA vault rather than sending a direct SPL transfer. After the gateway
//! proxies the request to the LLM provider, the EscrowClaimer fires a
//! fire-and-forget claim transaction to collect the actual cost from the vault.

#[cfg(feature = "postgres")]
pub mod claim_processor;
#[cfg(feature = "postgres")]
pub mod claim_queue;

mod claimer;
pub mod refund;
mod verifier;

// `deposit` and `pda` were extracted into the dependency-light `solvela-escrow-tx`
// crate so the Rust client SDK can reuse the on-chain-verified deposit-tx byte
// layout without pulling in x402's HTTP/Solana deps. Re-exported here so every
// existing internal reference (`super::pda::...`, `super::deposit::*`,
// `crate::escrow::deposit::*`) keeps resolving unchanged.
pub use solvela_escrow_tx::{deposit, pda};

#[cfg(feature = "postgres")]
pub use claim_processor::{EscrowMetrics, EscrowMetricsSnapshot};
pub use claimer::{do_claim_with_params, EscrowClaimer};
pub use verifier::EscrowVerifier;

#[cfg(test)]
mod reexport_tests {
    //! Tests that stay in x402 because they validate the re-exported
    //! `solvela-escrow-tx` types against x402-internal modules
    //! (`solana_types`) that the no-deps crate intentionally lacks, plus a
    //! byte-exact golden vector pinned identically in both crates.

    use super::deposit::{build_deposit_tx, DepositParams};
    use super::pda::{decode_bs58_pubkey, derive_ata_address};

    const PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

    /// Cross-check: the re-exported `pda::derive_ata_address` must agree with
    /// x402's own `solana_types::derive_ata` on the canonical on-chain ATA.
    /// Moved out of `solvela-escrow-tx` (which has no `solana_types`) so the
    /// re-export stays pinned to the same value.
    #[test]
    fn reexported_pda_matches_solana_types_derive_ata() {
        use crate::solana_types::{derive_ata as sol_derive_ata, Pubkey};

        let wallet_b = decode_bs58_pubkey("4P8mSmvv3nfzUtoqhNKG1mfGrHMVbXvKBXR7fDivv6qp")
            .expect("valid wallet");
        let mint_b =
            decode_bs58_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").expect("valid mint");

        // Re-exported escrow-tx derivation.
        let ata_escrow_tx = derive_ata_address(&wallet_b, &mint_b).expect("derivation");

        // x402 solana_types derivation.
        let ata_solana_types = sol_derive_ata(
            &Pubkey(wallet_b),
            &Pubkey(mint_b),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .expect("derivation");

        assert_eq!(
            bs58::encode(ata_escrow_tx).into_string(),
            ata_solana_types.to_string(),
            "re-exported escrow-tx ATA derivation must match solana_types"
        );
        assert_eq!(
            ata_solana_types.to_string(),
            "CYHVCkLwiEjMBdRiz5MsrrCbVL2YTZuv57TjV3ggxoSN"
        );
    }

    fn golden_keypair_b58() -> String {
        use ed25519_dalek::SigningKey;
        let seed = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    /// Byte-exact golden vector — MUST stay identical to the one in
    /// `solvela-escrow-tx`'s `golden_vector.rs`. Any drift in the deposit-tx
    /// byte layout (discriminator, account ordering, expiry math, signing)
    /// breaks both assertions. This is the money-path drift guard.
    #[test]
    fn deposit_tx_golden_vector() {
        let params = DepositParams {
            agent_keypair_b58: golden_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 2625,
            service_id: [7u8; 32],
            expiry_slot: 1_000_750,
            recent_blockhash: [0xABu8; 32],
        };
        let b64 = build_deposit_tx(&params).expect("golden build should succeed");
        assert_eq!(b64, solvela_escrow_tx::deposit::GOLDEN_VECTOR_B64);
    }
}
