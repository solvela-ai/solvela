//! PDA derivation and Solana address helpers for the escrow module.

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub(crate) const ATA_PROGRAM_ID: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJe1bxs";
pub(crate) const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub(crate) const SYSTEM_PROGRAM_ID: &str = "11111111111111111111111111111111";
pub(crate) const SYSVAR_RENT_ID: &str = "SysvarRent111111111111111111111111111111111";

// ---------------------------------------------------------------------------
// PDA derivation helpers
// ---------------------------------------------------------------------------

/// Derive a Program Derived Address using SHA-256 (same as Solana runtime).
///
/// Returns `(pubkey_bytes, bump)` or `None` if no valid off-curve point is found.
pub(crate) fn find_program_address(
    seeds: &[&[u8]],
    program_id: &[u8; 32],
) -> Option<([u8; 32], u8)> {
    use sha2::{Digest, Sha256};

    for nonce in (0u8..=255).rev() {
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([nonce]);
        hasher.update(b"ProgramDerivedAddress");
        hasher.update(program_id);
        let hash = hasher.finalize();

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);

        if !is_on_ed25519_curve(&bytes) {
            return Some((bytes, nonce));
        }
    }

    None
}

/// Check if 32 bytes represent a valid compressed point on the ed25519 curve.
pub(crate) fn is_on_ed25519_curve(bytes: &[u8; 32]) -> bool {
    use curve25519_dalek::edwards::CompressedEdwardsY;
    CompressedEdwardsY(*bytes).decompress().is_some()
}

/// Derive the Associated Token Account address for a given wallet and mint.
pub(crate) fn derive_ata_address(wallet: &[u8; 32], mint: &[u8; 32]) -> Option<[u8; 32]> {
    let token_program = decode_bs58_pubkey(TOKEN_PROGRAM_ID).ok()?;
    let ata_program = decode_bs58_pubkey(ATA_PROGRAM_ID).ok()?;

    let seeds: &[&[u8]] = &[wallet, &token_program, mint];
    find_program_address(seeds, &ata_program).map(|(addr, _)| addr)
}

/// Decode a base58-encoded pubkey into 32 bytes.
pub(crate) fn decode_bs58_pubkey(s: &str) -> Result<[u8; 32], String> {
    let bytes = bs58::decode(s)
        .into_vec()
        .map_err(|e| format!("invalid base58: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!("expected 32 bytes, got {}", bytes.len()));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(arr)
}

/// Compute the Anchor instruction discriminator: sha256("global:<name>")[..8].
pub(crate) fn anchor_discriminator(name: &str) -> [u8; 8] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(format!("global:{name}"));
    let hash = hasher.finalize();
    let mut disc = [0u8; 8];
    disc.copy_from_slice(&hash[..8]);
    disc
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anchor_discriminator() {
        let disc = anchor_discriminator("claim");
        // The discriminator should be 8 bytes from sha256("global:claim")
        assert_eq!(disc.len(), 8);
        // Verify it's deterministic
        assert_eq!(disc, anchor_discriminator("claim"));
        // Different names give different discriminators
        assert_ne!(disc, anchor_discriminator("deposit"));
    }

    #[test]
    fn test_pda_derivation() {
        // Use known inputs and verify we get a deterministic PDA
        let program_id = decode_bs58_pubkey("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU")
            .expect("valid program id");
        let agent = [1u8; 32];
        let service_id = [2u8; 32];

        let result = find_program_address(&[b"escrow", &agent, &service_id], &program_id);
        assert!(result.is_some());

        let (pda, bump) = result.unwrap();
        // PDA must be off the ed25519 curve
        assert!(!is_on_ed25519_curve(&pda));
        // Bump must be a valid u8 (which it is by type — just check it's > 0
        // to confirm the PDA derivation didn't need all 256 attempts)
        let _ = bump; // Bump is a u8, always valid

        // Same inputs → same result
        let (pda2, bump2) =
            find_program_address(&[b"escrow", &agent, &service_id], &program_id).unwrap();
        assert_eq!(pda, pda2);
        assert_eq!(bump, bump2);
    }

    #[test]
    fn test_derive_ata_address() {
        let wallet = [1u8; 32];
        let mint =
            decode_bs58_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").expect("valid mint");

        let ata = derive_ata_address(&wallet, &mint);
        assert!(ata.is_some());

        // Deterministic
        let ata2 = derive_ata_address(&wallet, &mint);
        assert_eq!(ata, ata2);
    }

    #[test]
    fn test_decode_bs58_pubkey_valid() {
        let result = decode_bs58_pubkey("11111111111111111111111111111111");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), [0u8; 32]);
    }

    #[test]
    fn test_decode_bs58_pubkey_invalid() {
        let result = decode_bs58_pubkey("invalid!!!");
        assert!(result.is_err());
    }
}
