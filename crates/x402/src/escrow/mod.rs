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
            agent_keypair_b58: zeroize::Zeroizing::new(golden_keypair_b58()),
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

    /// Independent-oracle round-trip: build a deposit tx with the client-side
    /// builder, then validate the resulting bytes using x402's *verifier-side*
    /// parsing and PDA/ATA re-derivation logic — the same logic
    /// `EscrowVerifier::verify_payment` runs, but exercised here WITHOUT live
    /// RPC (no `getSlot`; we replicate the verifier's pure structural checks).
    ///
    /// The golden vector above only asserts builder bytes against a frozen
    /// constant — it is one-sided and cannot catch *symmetric* drift where the
    /// builder and the verifier change together. This test closes that gap:
    /// builder-produced bytes are decoded and checked by independent
    /// verifier-side derivations (escrow PDA, vault ATA, agent ATA from the
    /// agent pubkey + service_id + program id), so a drift in account ordering,
    /// discriminator, amount, service_id, or expiry math fails here even if the
    /// golden constant were regenerated in lockstep.
    #[test]
    fn deposit_tx_validated_by_verifier_side_derivation() {
        use super::pda::{anchor_discriminator, find_program_address};
        use crate::solana_types::{derive_ata, Pubkey, VersionedTransaction};
        use ed25519_dalek::SigningKey;

        // --- Known params (distinct from the golden vector to avoid coupling) ---
        let seed = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let agent_pubkey = signing_key.verifying_key().to_bytes();

        let service_id = [9u8; 32];
        let amount: u64 = 7777;
        let expiry_slot: u64 = 1_234_567;

        let b64 = build_deposit_tx(&DepositParams {
            agent_keypair_b58: zeroize::Zeroizing::new(golden_keypair_b58()),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount,
            service_id,
            expiry_slot,
            recent_blockhash: [0x11u8; 32],
        })
        .expect("deposit build should succeed");

        // --- Decode the builder bytes via the verifier's own tx parser ---
        use base64::Engine;
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("valid base64");
        let tx = VersionedTransaction::from_bytes(&tx_bytes).expect("decodes as versioned tx");
        let message = tx.parse_message().expect("message parses");

        // --- Independently re-derive every account the verifier re-derives ---
        let program_id = decode_bs58_pubkey(ESCROW_PROGRAM).expect("program id");
        let usdc_mint = decode_bs58_pubkey(USDC_MINT).expect("mint");
        let provider = decode_bs58_pubkey(PROVIDER).expect("provider");
        let (escrow_pda, _bump) =
            find_program_address(&[b"escrow", &agent_pubkey, &service_id], &program_id)
                .expect("escrow PDA");
        let agent_ata = derive_ata(
            &Pubkey(agent_pubkey),
            &Pubkey(usdc_mint),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .expect("agent ATA");
        let vault_ata = derive_ata(
            &Pubkey(escrow_pda),
            &Pubkey(usdc_mint),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .expect("vault ATA");

        // --- Locate the escrow `deposit` instruction (verifier-side search) ---
        let deposit_ix = message
            .instructions
            .iter()
            .find(|ix| {
                message
                    .account_keys
                    .get(ix.program_id_index as usize)
                    .map(|k| k.0 == program_id)
                    .unwrap_or(false)
            })
            .expect("escrow deposit instruction present");

        // --- Discriminator + instruction-data fields ---
        let expected_disc = anchor_discriminator("deposit");
        assert_eq!(
            deposit_ix.data.len(),
            56,
            "deposit ix data must be 56 bytes"
        );
        assert_eq!(&deposit_ix.data[..8], &expected_disc, "discriminator drift");

        let ix_amount = u64::from_le_bytes(deposit_ix.data[8..16].try_into().unwrap());
        assert_eq!(
            ix_amount, amount,
            "encoded amount must equal builder amount"
        );

        let ix_service_id: [u8; 32] = deposit_ix.data[16..48].try_into().unwrap();
        assert_eq!(ix_service_id, service_id, "service_id drift");

        let ix_expiry = u64::from_le_bytes(deposit_ix.data[48..56].try_into().unwrap());
        assert_eq!(ix_expiry, expiry_slot, "expiry_slot drift");

        // --- Account layout: positions must map to the re-derived accounts ---
        // Verifier layout: 0=agent, 1=provider, 2=mint, 3=escrow, 4=agent_ata, 5=vault.
        let key_at = |pos: usize| -> [u8; 32] {
            let idx = deposit_ix.accounts[pos] as usize;
            message.account_keys[idx].0
        };
        assert_eq!(key_at(0), agent_pubkey, "ix account 0 must be agent");
        assert_eq!(key_at(1), provider, "ix account 1 must be provider");
        assert_eq!(key_at(2), usdc_mint, "ix account 2 must be mint");
        assert_eq!(key_at(3), escrow_pda, "ix account 3 must be escrow PDA");
        assert_eq!(key_at(4), agent_ata.0, "ix account 4 must be agent ATA");
        assert_eq!(key_at(5), vault_ata.0, "ix account 5 must be vault ATA");

        // Program key must be the escrow program (consistency with the search).
        assert_eq!(
            message.account_keys[deposit_ix.program_id_index as usize].0, program_id,
            "program id account must be the escrow program",
        );
    }
}
