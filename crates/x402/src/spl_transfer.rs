//! Shared SPL Token transfer extraction logic.
//!
//! Used by both `solana.rs` (direct payments) and `escrow/` (deposit verification)
//! to parse SPL Token Transfer and TransferChecked instructions from a transaction.

use crate::solana_types::{ParsedMessage, Pubkey};
use crate::traits::Error;

/// Information extracted from an SPL Token transfer instruction.
#[derive(Debug, Clone)]
pub(crate) struct SplTransferInfo {
    /// The destination token account.
    pub destination: Pubkey,
    /// Transfer amount in atomic units.
    pub amount: u64,
    /// Mint address (only present for TransferChecked).
    pub mint: Option<Pubkey>,
}

/// Extract SPL Token transfer information from a parsed message.
///
/// Searches for SPL Token `Transfer` (discriminator 3) or `TransferChecked`
/// (discriminator 12) instructions. Returns the first matching transfer.
pub(crate) fn extract_spl_transfer(message: &ParsedMessage) -> Result<SplTransferInfo, Error> {
    for ix in &message.instructions {
        let program_id_index = ix.program_id_index as usize;
        if program_id_index >= message.account_keys.len() {
            continue;
        }

        let program_id = &message.account_keys[program_id_index];

        // Check if this is an SPL Token program instruction
        let is_token_program =
            *program_id == Pubkey::TOKEN_PROGRAM_ID || *program_id == Pubkey::TOKEN_2022_PROGRAM_ID;
        if !is_token_program {
            continue;
        }

        if ix.data.is_empty() {
            continue;
        }

        match ix.data[0] {
            // Transfer: discriminator=3, data[1..9]=amount(u64 LE)
            // accounts: [source, destination, authority]
            3 => {
                if ix.data.len() < 9 {
                    return Err(Error::InvalidTransaction(
                        "SPL Transfer instruction data too short".to_string(),
                    ));
                }
                if ix.accounts.len() < 2 {
                    return Err(Error::InvalidTransaction(
                        "SPL Transfer instruction missing accounts".to_string(),
                    ));
                }

                let amount = u64::from_le_bytes(ix.data[1..9].try_into().map_err(|_| {
                    Error::InvalidTransaction("failed to parse transfer amount".to_string())
                })?);

                let dest_index = ix.accounts[1] as usize;
                if dest_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "destination account index out of bounds".to_string(),
                    ));
                }
                let destination = message.account_keys[dest_index];

                return Ok(SplTransferInfo {
                    destination,
                    amount,
                    mint: None,
                });
            }
            // TransferChecked: discriminator=12, data[1..9]=amount(u64 LE)
            // accounts: [source, mint, destination, authority]
            12 => {
                if ix.data.len() < 9 {
                    return Err(Error::InvalidTransaction(
                        "SPL TransferChecked instruction data too short".to_string(),
                    ));
                }
                if ix.accounts.len() < 3 {
                    return Err(Error::InvalidTransaction(
                        "SPL TransferChecked instruction missing accounts".to_string(),
                    ));
                }

                let amount = u64::from_le_bytes(ix.data[1..9].try_into().map_err(|_| {
                    Error::InvalidTransaction("failed to parse transfer amount".to_string())
                })?);

                let mint_index = ix.accounts[1] as usize;
                if mint_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "mint account index out of bounds".to_string(),
                    ));
                }
                let mint = message.account_keys[mint_index];

                let dest_index = ix.accounts[2] as usize;
                if dest_index >= message.account_keys.len() {
                    return Err(Error::InvalidTransaction(
                        "destination account index out of bounds".to_string(),
                    ));
                }
                let destination = message.account_keys[dest_index];

                return Ok(SplTransferInfo {
                    destination,
                    amount,
                    mint: Some(mint),
                });
            }
            _ => continue,
        }
    }

    Err(Error::InvalidTransaction(
        "no SPL Token transfer instruction found".to_string(),
    ))
}
