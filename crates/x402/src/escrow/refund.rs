//! Escrow refund transaction builder for client SDKs.
//!
//! Builds a signed Solana legacy transaction that calls the escrow program's
//! `refund` instruction. The transaction is returned as a base64-encoded string
//! suitable for submission to any Solana RPC endpoint.
//!
//! The `refund` instruction closes the escrow PDA and returns all deposited
//! tokens (plus rent) to the agent's wallet. It is only callable after the
//! escrow's `expiry_slot` has passed.

use super::pda::{
    anchor_discriminator, decode_bs58_pubkey, derive_ata_address, find_program_address,
    ATA_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors that can occur while building an escrow refund transaction.
#[derive(Debug, thiserror::Error)]
pub enum RefundError {
    /// The agent keypair was invalid (bad base58, wrong length, or pubkey mismatch).
    #[error("invalid agent keypair: {0}")]
    InvalidKeypair(String),
    /// An address field failed to decode from base58.
    #[error("invalid address for {field}: {reason}")]
    InvalidAddress { field: &'static str, reason: String },
    /// A PDA or ATA derivation failed.
    #[error("failed to derive {0}")]
    DerivationFailed(&'static str),
    /// The caller-provided PDA does not match the one derived from the
    /// agent pubkey, service_id, and program id.
    #[error("derived PDA {derived} does not match expected {expected}")]
    PdaMismatch { derived: String, expected: String },
}

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Parameters required to build an escrow refund transaction.
pub struct RefundParams {
    /// Base58-encoded 64-byte ed25519 keypair (32-byte secret || 32-byte pubkey).
    pub agent_keypair_b58: String,
    /// Base58-encoded escrow PDA pubkey. Re-derived from the agent + service_id
    /// for safety; the caller-provided value must match.
    pub escrow_pda_b58: String,
    /// Base58-encoded USDC mint pubkey.
    pub usdc_mint_b58: String,
    /// Base58-encoded escrow program ID.
    pub escrow_program_id_b58: String,
    /// 32-byte service identifier that seeded the escrow PDA.
    pub service_id: [u8; 32],
    /// Recent blockhash (32 bytes) from `getLatestBlockhash`.
    pub recent_blockhash: [u8; 32],
}

impl std::fmt::Debug for RefundParams {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RefundParams")
            .field("agent_keypair_b58", &"[REDACTED]")
            .field("escrow_pda_b58", &self.escrow_pda_b58)
            .field("usdc_mint_b58", &self.usdc_mint_b58)
            .field("escrow_program_id_b58", &self.escrow_program_id_b58)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// Transaction builder
// ---------------------------------------------------------------------------

/// Build a signed escrow refund transaction.
///
/// Returns a base64-encoded wire-format Solana transaction ready for
/// submission via `sendTransaction`.
///
/// # Errors
///
/// Returns [`RefundError`] if:
/// - The keypair is invalid or the derived pubkey does not match the stored one
/// - Any address fails to decode from base58
/// - PDA or ATA derivation fails
/// - The caller-provided `escrow_pda_b58` does not match the PDA derived from
///   the agent pubkey, service_id, and program id
pub fn build_refund_tx(params: &RefundParams) -> Result<String, RefundError> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    // Step 1: Decode the 64-byte keypair and validate derived pubkey
    let keypair_bytes = bs58::decode(&params.agent_keypair_b58)
        .into_vec()
        .map_err(|e| RefundError::InvalidKeypair(format!("invalid base58: {e}")))?;
    if keypair_bytes.len() != 64 {
        return Err(RefundError::InvalidKeypair(format!(
            "must be 64 bytes, got {}",
            keypair_bytes.len()
        )));
    }
    let mut keypair_arr = [0u8; 64];
    keypair_arr.copy_from_slice(&keypair_bytes);

    let signing_key = SigningKey::from_keypair_bytes(&keypair_arr)
        .map_err(|e| RefundError::InvalidKeypair(format!("ed25519 error: {e}")))?;
    let agent_pubkey = signing_key.verifying_key().to_bytes();

    // Validate that the stored pubkey in bytes 32..64 matches the derived one.
    let stored_pubkey = &keypair_bytes[32..64];
    if stored_pubkey != agent_pubkey {
        return Err(RefundError::InvalidKeypair(
            "derived pubkey does not match stored pubkey".to_string(),
        ));
    }

    // Step 2: Parse all addresses
    let expected_escrow_pda =
        decode_bs58_pubkey(&params.escrow_pda_b58).map_err(|e| RefundError::InvalidAddress {
            field: "escrow_pda",
            reason: e.to_string(),
        })?;
    let usdc_mint =
        decode_bs58_pubkey(&params.usdc_mint_b58).map_err(|e| RefundError::InvalidAddress {
            field: "usdc_mint",
            reason: e.to_string(),
        })?;
    let escrow_program_id = decode_bs58_pubkey(&params.escrow_program_id_b58).map_err(|e| {
        RefundError::InvalidAddress {
            field: "escrow_program_id",
            reason: e.to_string(),
        }
    })?;
    let token_program =
        decode_bs58_pubkey(TOKEN_PROGRAM_ID).map_err(|e| RefundError::InvalidAddress {
            field: "token_program",
            reason: e.to_string(),
        })?;
    let ata_program =
        decode_bs58_pubkey(ATA_PROGRAM_ID).map_err(|e| RefundError::InvalidAddress {
            field: "ata_program",
            reason: e.to_string(),
        })?;
    let system_program =
        decode_bs58_pubkey(SYSTEM_PROGRAM_ID).map_err(|e| RefundError::InvalidAddress {
            field: "system_program",
            reason: e.to_string(),
        })?;

    // Step 3: Re-derive the escrow PDA and compare to the caller-provided PDA.
    // This catches caller bugs (e.g. wrong service_id or wrong program id).
    let (derived_escrow_pda, _bump) = find_program_address(
        &[b"escrow", &agent_pubkey, &params.service_id],
        &escrow_program_id,
    )
    .ok_or(RefundError::DerivationFailed("escrow PDA"))?;

    if derived_escrow_pda != expected_escrow_pda {
        return Err(RefundError::PdaMismatch {
            derived: bs58::encode(derived_escrow_pda).into_string(),
            expected: params.escrow_pda_b58.clone(),
        });
    }

    // Step 4: Derive agent ATA and vault ATA
    let agent_token_account = derive_ata_address(&agent_pubkey, &usdc_mint)
        .ok_or(RefundError::DerivationFailed("agent ATA"))?;
    let vault_ata = derive_ata_address(&derived_escrow_pda, &usdc_mint)
        .ok_or(RefundError::DerivationFailed("vault ATA"))?;

    // Step 5: Build account keys sorted by writability (Solana legacy message requirement):
    //   writable signers first, then writable non-signers, then readonly non-signers.
    // 0: agent_pubkey         (signer, writable)
    // 1: escrow_pda           (writable, non-signer — closed to agent)
    // 2: vault_ata            (writable, non-signer — closed by CPI)
    // 3: agent_token_account  (writable, non-signer — receives refund)
    // 4: usdc_mint            (readonly, non-signer)
    // 5: token_program        (readonly, non-signer)
    // 6: ata_program          (readonly, non-signer)
    // 7: system_program       (readonly, non-signer)
    // 8: escrow_program_id    (program invoked — appended last)
    let accounts: Vec<[u8; 32]> = vec![
        agent_pubkey,        // 0: signer, writable
        derived_escrow_pda,  // 1: writable, non-signer
        vault_ata,           // 2: writable, non-signer
        agent_token_account, // 3: writable, non-signer
        usdc_mint,           // 4: readonly
        token_program,       // 5: readonly
        ata_program,         // 6: readonly
        system_program,      // 7: readonly
                             // escrow_program_id appended separately as index 8
    ];

    // Step 6: Build instruction data
    // refund takes no arguments — just the discriminator.
    let discriminator = anchor_discriminator("refund");
    let ix_data = discriminator.to_vec();
    debug_assert_eq!(ix_data.len(), 8, "refund ix_data must be 8 bytes");

    // Instruction account indices remapped to the sorted wire order.
    // Anchor program expects (Refund<'info> struct field order):
    //   [escrow, agent, mint, vault, agent_token_account,
    //    token_program, associated_token_program, system_program]
    // Wire positions:
    //   [1,      0,     4,    2,     3,
    //    5,             6,                        7]
    let ix_account_indices: Vec<u8> = vec![1, 0, 4, 2, 3, 5, 6, 7];

    // Step 7: Build the legacy message
    // Header: [1, 0, 5] — 1 signer, 0 readonly signed, 5 readonly unsigned
    // Readonly unsigned: mint(4), token_program(5), ata_program(6),
    //                    system_program(7), escrow_program(8) = 5
    let msg = super::deposit::build_legacy_message(
        [1, 0, 5],
        &accounts,
        &escrow_program_id,
        &params.recent_blockhash,
        8u8, // program_id_index = 8 (last account)
        &ix_account_indices,
        &ix_data,
    );

    // Step 8: Sign the message with the agent keypair
    let signature = signing_key.sign(&msg);

    // Step 9: Assemble wire-format transaction
    // compact-u16(1) || signature(64) || message
    let mut tx_bytes = Vec::with_capacity(1 + 64 + msg.len());
    tx_bytes.push(0x01); // compact-u16: 1 signature
    tx_bytes.extend_from_slice(&signature.to_bytes());
    tx_bytes.extend_from_slice(&msg);

    // Step 10: Base64-encode and return
    Ok(base64::engine::general_purpose::STANDARD.encode(&tx_bytes))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine as _;
    use ed25519_dalek::SigningKey;

    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

    /// Build a deterministic 64-byte keypair from seed `[42u8; 32]`.
    fn test_agent_keypair_b58() -> String {
        let seed = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let mut keypair_bytes = [0u8; 64];
        keypair_bytes[..32].copy_from_slice(&signing_key.to_bytes());
        keypair_bytes[32..].copy_from_slice(signing_key.verifying_key().as_bytes());
        bs58::encode(&keypair_bytes).into_string()
    }

    /// Compute the expected escrow PDA for the test keypair + service_id.
    fn expected_escrow_pda_b58(service_id: &[u8; 32]) -> String {
        let seed = [42u8; 32];
        let signing_key = SigningKey::from_bytes(&seed);
        let agent_pubkey = signing_key.verifying_key().to_bytes();
        let program_id = decode_bs58_pubkey(ESCROW_PROGRAM).expect("valid program id");
        let (pda, _bump) =
            find_program_address(&[b"escrow", &agent_pubkey, service_id], &program_id)
                .expect("valid PDA");
        bs58::encode(pda).into_string()
    }

    fn base_params() -> RefundParams {
        let service_id = [1u8; 32];
        RefundParams {
            agent_keypair_b58: test_agent_keypair_b58(),
            escrow_pda_b58: expected_escrow_pda_b58(&service_id),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            service_id,
            recent_blockhash: [0xABu8; 32],
        }
    }

    #[test]
    fn test_build_refund_tx_produces_valid_base64() {
        let result = build_refund_tx(&base_params());
        assert!(result.is_ok(), "build_refund_tx failed: {:?}", result.err());
        let b64 = result.unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("output must be valid base64");
        assert!(
            decoded.len() > 100,
            "transaction too short: {} bytes",
            decoded.len()
        );
    }

    #[test]
    fn test_build_refund_tx_contains_correct_discriminator() {
        let b64 = build_refund_tx(&base_params()).expect("build should succeed");
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();
        let disc = anchor_discriminator("refund");
        let found = tx_bytes.windows(8).any(|w| w == disc);
        assert!(
            found,
            "transaction bytes must contain the refund discriminator"
        );
    }

    #[test]
    fn test_build_refund_tx_contains_agent_pubkey() {
        let keypair_b58 = test_agent_keypair_b58();
        let keypair_bytes = bs58::decode(&keypair_b58).into_vec().unwrap();
        let agent_pubkey = &keypair_bytes[32..64]; // stored pubkey portion

        let b64 = build_refund_tx(&base_params()).expect("build should succeed");
        let tx_bytes = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .unwrap();

        let found = tx_bytes.windows(32).any(|w| w == agent_pubkey);
        assert!(found, "transaction bytes must contain the agent pubkey");
    }

    #[test]
    fn test_build_refund_tx_pda_mismatch_rejected() {
        let mut params = base_params();
        // Replace with a valid base58 pubkey that is definitely not the
        // derived PDA (system program).
        params.escrow_pda_b58 = "11111111111111111111111111111111".to_string();
        let result = build_refund_tx(&params);
        assert!(result.is_err(), "wrong PDA should be rejected");
        let err = result.unwrap_err();
        assert!(
            matches!(err, RefundError::PdaMismatch { .. }),
            "expected PdaMismatch variant, got: {err}"
        );
    }

    #[test]
    fn test_build_refund_tx_invalid_keypair_rejected() {
        let mut params = base_params();
        params.agent_keypair_b58 = "notavalidkeypair!!!".to_string();
        let result = build_refund_tx(&params);
        assert!(result.is_err(), "invalid keypair should be rejected");
        let err = result.unwrap_err();
        assert!(
            matches!(err, RefundError::InvalidKeypair(_)),
            "expected InvalidKeypair variant, got: {err}"
        );
    }

    #[test]
    fn test_refund_params_debug_redacts_keypair() {
        let params = base_params();
        let debug_str = format!("{params:?}");
        assert!(
            debug_str.contains("[REDACTED]"),
            "Debug output must redact the keypair: {debug_str}"
        );
        assert!(
            !debug_str.contains(&params.agent_keypair_b58),
            "Debug output must not contain the raw keypair: {debug_str}"
        );
    }
}
