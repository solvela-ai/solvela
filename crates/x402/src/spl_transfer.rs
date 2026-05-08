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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_types::{CompiledInstruction, MessageHeader};

    fn pubkey_from_byte(byte: u8) -> Pubkey {
        let mut bytes = [0u8; 32];
        bytes[0] = byte;
        Pubkey(bytes)
    }

    fn empty_message_with(
        account_keys: Vec<Pubkey>,
        instructions: Vec<CompiledInstruction>,
    ) -> ParsedMessage {
        ParsedMessage {
            header: MessageHeader {
                num_required_signatures: 1,
                num_readonly_signed_accounts: 0,
                num_readonly_unsigned_accounts: 0,
            },
            account_keys,
            recent_blockhash: [0u8; 32],
            instructions,
            is_v0: false,
        }
    }

    fn transfer_data(amount: u64) -> Vec<u8> {
        let mut data = vec![3];
        data.extend_from_slice(&amount.to_le_bytes());
        data
    }

    fn transfer_checked_data(amount: u64, decimals: u8) -> Vec<u8> {
        let mut data = vec![12];
        data.extend_from_slice(&amount.to_le_bytes());
        data.push(decimals);
        data
    }

    #[test]
    fn extracts_transfer_amount_and_destination() {
        let source = pubkey_from_byte(1);
        let destination = pubkey_from_byte(2);
        let authority = pubkey_from_byte(3);
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        let message = empty_message_with(
            vec![source, destination, authority, token_program],
            vec![CompiledInstruction {
                program_id_index: 3,
                accounts: vec![0, 1, 2],
                data: transfer_data(1_000_000),
            }],
        );

        let info = extract_spl_transfer(&message).expect("transfer must parse");
        assert_eq!(info.amount, 1_000_000);
        assert_eq!(info.destination, destination);
        assert!(info.mint.is_none(), "Transfer has no mint field");
    }

    #[test]
    fn extracts_transfer_checked_amount_destination_and_mint() {
        let source = pubkey_from_byte(1);
        let mint = pubkey_from_byte(2);
        let destination = pubkey_from_byte(3);
        let authority = pubkey_from_byte(4);
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        let message = empty_message_with(
            vec![source, mint, destination, authority, token_program],
            vec![CompiledInstruction {
                program_id_index: 4,
                accounts: vec![0, 1, 2, 3],
                data: transfer_checked_data(2_500_000, 6),
            }],
        );

        let info = extract_spl_transfer(&message).expect("transfer-checked must parse");
        assert_eq!(info.amount, 2_500_000);
        assert_eq!(info.destination, destination);
        assert_eq!(info.mint, Some(mint));
    }

    #[test]
    fn accepts_transfer_checked_from_token_2022_program() {
        let source = pubkey_from_byte(1);
        let mint = pubkey_from_byte(2);
        let destination = pubkey_from_byte(3);
        let authority = pubkey_from_byte(4);
        let token2022_program = Pubkey::TOKEN_2022_PROGRAM_ID;

        let message = empty_message_with(
            vec![source, mint, destination, authority, token2022_program],
            vec![CompiledInstruction {
                program_id_index: 4,
                accounts: vec![0, 1, 2, 3],
                data: transfer_checked_data(7, 9),
            }],
        );

        let info = extract_spl_transfer(&message).expect("token-2022 transfer must parse");
        assert_eq!(info.amount, 7);
        assert_eq!(info.mint, Some(mint));
    }

    #[test]
    fn skips_non_spl_token_program_instructions() {
        let source = pubkey_from_byte(1);
        let destination = pubkey_from_byte(2);
        let authority = pubkey_from_byte(3);
        let other_program = pubkey_from_byte(99);

        let message = empty_message_with(
            vec![source, destination, authority, other_program],
            vec![CompiledInstruction {
                program_id_index: 3,
                accounts: vec![0, 1, 2],
                data: transfer_data(1),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("must reject non-token program");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("no SPL Token transfer"), "wrong error: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn skips_empty_instruction_data() {
        let source = pubkey_from_byte(1);
        let destination = pubkey_from_byte(2);
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        let message = empty_message_with(
            vec![source, destination, token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: vec![],
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("empty data must not match");
        assert!(matches!(err, Error::InvalidTransaction(_)));
    }

    #[test]
    fn skips_out_of_bounds_program_id_index() {
        let source = pubkey_from_byte(1);
        let destination = pubkey_from_byte(2);

        let message = empty_message_with(
            vec![source, destination],
            vec![CompiledInstruction {
                program_id_index: 99,
                accounts: vec![0, 1],
                data: transfer_data(1),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("oob program index must not match");
        assert!(matches!(err, Error::InvalidTransaction(_)));
    }

    #[test]
    fn rejects_transfer_with_truncated_data() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), pubkey_from_byte(2), token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: vec![3, 0, 0],
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("truncated data must error");
        match err {
            Error::InvalidTransaction(msg) => assert!(msg.contains("too short"), "msg: {msg}"),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_with_missing_accounts() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), token_program],
            vec![CompiledInstruction {
                program_id_index: 1,
                accounts: vec![0],
                data: transfer_data(100),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("missing accounts must error");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("missing accounts"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_with_oob_destination_index() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), pubkey_from_byte(2), token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 99, 1],
                data: transfer_data(100),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("oob destination must error");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("destination account index"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_checked_with_truncated_data() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), pubkey_from_byte(2), token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1, 0],
                data: vec![12, 0, 0],
            }],
        );

        let err =
            extract_spl_transfer(&message).expect_err("truncated transfer-checked must error");
        match err {
            Error::InvalidTransaction(msg) => assert!(msg.contains("too short"), "msg: {msg}"),
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_checked_with_missing_accounts() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), pubkey_from_byte(2), token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: transfer_checked_data(100, 6),
            }],
        );

        let err = extract_spl_transfer(&message)
            .expect_err("missing accounts in transfer-checked must error");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("missing accounts"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_checked_with_oob_mint_index() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![
                pubkey_from_byte(1),
                pubkey_from_byte(2),
                pubkey_from_byte(3),
                token_program,
            ],
            vec![CompiledInstruction {
                program_id_index: 3,
                accounts: vec![0, 99, 2, 0],
                data: transfer_checked_data(100, 6),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("oob mint must error");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("mint account index"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_transfer_checked_with_oob_destination_index() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![
                pubkey_from_byte(1),
                pubkey_from_byte(2),
                pubkey_from_byte(3),
                token_program,
            ],
            vec![CompiledInstruction {
                program_id_index: 3,
                accounts: vec![0, 1, 99, 0],
                data: transfer_checked_data(100, 6),
            }],
        );

        let err = extract_spl_transfer(&message).expect_err("oob destination must error");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("destination account index"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn skips_unknown_discriminators() {
        let token_program = Pubkey::TOKEN_PROGRAM_ID;
        let message = empty_message_with(
            vec![pubkey_from_byte(1), pubkey_from_byte(2), token_program],
            vec![CompiledInstruction {
                program_id_index: 2,
                accounts: vec![0, 1],
                data: vec![7, 0, 0, 0, 0, 0, 0, 0, 0],
            }],
        );

        let err = extract_spl_transfer(&message)
            .expect_err("non-transfer discriminator must produce 'no transfer found'");
        match err {
            Error::InvalidTransaction(msg) => {
                assert!(msg.contains("no SPL Token transfer"), "msg: {msg}")
            }
            other => panic!("unexpected error variant: {other:?}"),
        }
    }

    #[test]
    fn finds_transfer_after_skipping_non_token_instruction() {
        let source = pubkey_from_byte(1);
        let destination = pubkey_from_byte(2);
        let other_program = pubkey_from_byte(99);
        let token_program = Pubkey::TOKEN_PROGRAM_ID;

        let message = empty_message_with(
            vec![source, destination, other_program, token_program],
            vec![
                CompiledInstruction {
                    program_id_index: 2,
                    accounts: vec![],
                    data: vec![1, 2, 3],
                },
                CompiledInstruction {
                    program_id_index: 3,
                    accounts: vec![0, 1, 0],
                    data: transfer_data(42),
                },
            ],
        );

        let info = extract_spl_transfer(&message).expect("must find the second instruction");
        assert_eq!(info.amount, 42);
        assert_eq!(info.destination, destination);
    }
}
