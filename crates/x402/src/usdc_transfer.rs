//! Production USDC-SPL transfer builder + signer for the channel refund
//! disbursement: `CreateIdempotent(destination ATA)` + `TransferChecked` in one
//! legacy transaction, signed by the source owner (the gateway's operational
//! wallet).
//!
//! Byte-exact, golden-vector-pinned (see the tests — the expected bytes are
//! hand-assembled from the documented specs, never from this builder's own
//! output). Mirrors the framing of [`crate::solana::build_system_transfer_message`]:
//! single-byte compact-u16 length prefixes (all lengths here are far below
//! 128), accounts sorted by writability, program keys in the readonly tail.
//!
//! `CreateIdempotent` is prepended (plan Decision F) so a destination that has
//! closed its ATA cannot permanently strand the refund — the instruction is a
//! no-op when the ATA already exists. Layout verified against the canonical
//! `solana-program/associated-token-account` interface (2026-07-03): data =
//! `[1]`, accounts = `[funder (ws), ata (w), wallet, mint, system, token]`.
//!
//! Money-path rules (solvela-x402 §6): integer atomic amounts only, zero
//! amounts rejected, the signer keypair validated against the declared source
//! owner (never sign for a mismatched fee payer), key bytes zeroized by the
//! caller (the gateway holds them in a [`crate::fee_payer::FeePayerWallet`]).

use crate::solana_types::{derive_ata, Pubkey, ASSOCIATED_TOKEN_PROGRAM_ID};

/// USDC has 6 decimals; `TransferChecked` re-asserts this on-chain.
///
/// ponytail: constant 6 decimals AND the legacy token program — the real
/// ceiling here is the token PROGRAM, not just decimals: the ATA derivation
/// and the TransferChecked below both hardcode `TOKEN_PROGRAM_ID`, so a
/// Token-2022 mint (the planted-flag PYUSD-SPL is Token-2022) needs the token
/// program id AND decimals stored per reservation, not widened defaults.
/// Unreachable today: channel opens pin the config USDC mint per channel.
const USDC_DECIMALS: u8 = 6;

/// SPL Token `TransferChecked` instruction discriminator.
const TRANSFER_CHECKED_DISCRIMINATOR: u8 = 12;

/// SPL Associated Token Account program `CreateIdempotent` instruction byte.
const CREATE_IDEMPOTENT_DISCRIMINATOR: u8 = 1;

/// Errors from building or signing a USDC `TransferChecked` transaction.
/// Every variant is a hard failure — no path builds a zero/partial transfer.
#[derive(Debug, thiserror::Error)]
pub enum UsdcTransferError {
    /// The transfer amount was zero atomic units.
    #[error("transfer amount must be greater than zero atomic units")]
    ZeroAmount,
    /// The signing keypair was invalid (bad length or ed25519 error).
    #[error("invalid source keypair: {0}")]
    InvalidKeypair(String),
    /// The derived signer pubkey did not match the supplied source owner.
    #[error("source keypair pubkey does not match the source owner")]
    KeypairMismatch,
    /// An associated token account could not be derived (no valid bump).
    #[error("could not derive the {0} associated token account")]
    AtaDerivation(&'static str),
}

/// A signed USDC transfer, ready for broadcast and durable storage.
#[derive(Debug, Clone)]
pub struct SignedUsdcTransfer {
    /// The exact wire bytes (`compact-u16(1) || signature || message`). Persist
    /// these; every rebroadcast MUST reuse them verbatim so the transaction
    /// signature stays the ledger-level dedupe key.
    pub wire_bytes: Vec<u8>,
    /// Base64 (standard alphabet) of `wire_bytes`, the `sendTransaction` form.
    pub base64_tx: String,
    /// Base58 of the ed25519 signature — the transaction signature.
    pub signature_b58: String,
}

/// Build a Solana **legacy** message that transfers `amount` atomic units of
/// `mint` from `owner`'s ATA to `destination_wallet`'s ATA, creating the
/// destination ATA idempotently first.
///
/// Both ATAs are derived in here from `(wallet, mint)` — the caller supplies
/// wallets and the mint (for a refund: the FROZEN reservation tuple), never a
/// raw token-account address that could point anywhere.
///
/// Layout (all length prefixes single-byte compact-u16):
/// - header `[1, 0, 5]` — 1 signer, 0 readonly-signed, 5 readonly-unsigned.
/// - account keys (8):
///   - `0: owner`              (signer, writable — fee payer + token authority
///     + ATA-create funder)
///   - `1: source_ata`         (writable)
///   - `2: destination_ata`    (writable)
///   - `3: destination_wallet` (readonly — ATA-create wallet)
///   - `4: mint`               (readonly)
///   - `5: system_program`     (readonly)
///   - `6: token_program`      (readonly)
///   - `7: ata_program`        (readonly)
/// - 32-byte recent blockhash
/// - instruction count `2`
/// - instruction 1 — `CreateIdempotent` (program index 7): accounts
///   `[0, 2, 3, 4, 5, 6]`, data `[1]`.
/// - instruction 2 — `TransferChecked` (program index 6): accounts
///   `[1, 4, 2, 0]` (source, mint, destination, authority), data
///   `[12] ++ u64_le(amount) ++ [6]`.
pub fn build_usdc_transfer_checked_message(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, UsdcTransferError> {
    if amount == 0 {
        return Err(UsdcTransferError::ZeroAmount);
    }

    let owner_pk = Pubkey(*owner);
    let destination_pk = Pubkey(*destination_wallet);
    let mint_pk = Pubkey(*mint);

    let source_ata = derive_ata(&owner_pk, &mint_pk, &Pubkey::TOKEN_PROGRAM_ID)
        .ok_or(UsdcTransferError::AtaDerivation("source"))?;
    let destination_ata = derive_ata(&destination_pk, &mint_pk, &Pubkey::TOKEN_PROGRAM_ID)
        .ok_or(UsdcTransferError::AtaDerivation("destination"))?;

    let system_program = [0u8; 32]; // 11111111111111111111111111111111

    let mut msg = Vec::with_capacity(320);

    // Header: 1 required signature, 0 readonly-signed, 5 readonly-unsigned.
    msg.extend_from_slice(&[1u8, 0u8, 5u8]);

    // Account keys (8 total). Single-byte compact-u16 count.
    msg.push(8u8);
    msg.extend_from_slice(owner);
    msg.extend_from_slice(&source_ata.0);
    msg.extend_from_slice(&destination_ata.0);
    msg.extend_from_slice(destination_wallet);
    msg.extend_from_slice(mint);
    msg.extend_from_slice(&system_program);
    msg.extend_from_slice(&Pubkey::TOKEN_PROGRAM_ID.0);
    msg.extend_from_slice(&ASSOCIATED_TOKEN_PROGRAM_ID.0);

    // Recent blockhash.
    msg.extend_from_slice(recent_blockhash);

    // Instruction count: 2.
    msg.push(2u8);

    // Instruction 1: CreateIdempotent (ATA program).
    // Accounts: [funder, ata, wallet, mint, system_program, token_program].
    msg.push(7u8); // program_id_index = ata_program
    msg.push(6u8); // account index count
    msg.extend_from_slice(&[0u8, 2u8, 3u8, 4u8, 5u8, 6u8]);
    msg.push(1u8); // data length
    msg.push(CREATE_IDEMPOTENT_DISCRIMINATOR);

    // Instruction 2: TransferChecked (token program).
    // Accounts: [source, mint, destination, authority].
    let mut ix_data = Vec::with_capacity(10);
    ix_data.push(TRANSFER_CHECKED_DISCRIMINATOR);
    ix_data.extend_from_slice(&amount.to_le_bytes());
    ix_data.push(USDC_DECIMALS);
    msg.push(6u8); // program_id_index = token_program
    msg.push(4u8); // account index count
    msg.extend_from_slice(&[1u8, 4u8, 2u8, 0u8]);
    msg.push(ix_data.len() as u8); // data length (10)
    msg.extend_from_slice(&ix_data);

    Ok(msg)
}

/// Sign a USDC `CreateIdempotent + TransferChecked` message with the source
/// owner's ed25519 keypair and assemble the wire transaction.
///
/// `owner_keypair` is the raw 64-byte ed25519 keypair (`secret[0..32] ||
/// pubkey[32..64]`). The owner MUST be `account_keys[0]` (fee payer, token
/// authority, and ATA-create funder) — the derived signer pubkey is checked
/// against `owner` so a tampered or mismatched key is rejected rather than
/// signing over the wrong fee payer (the [`crate::solana::sign_system_transfer`]
/// guard, and the module-level payer==payee rule the gateway layers on top).
pub fn sign_usdc_transfer_checked(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    owner_keypair: &[u8; 64],
) -> Result<SignedUsdcTransfer, UsdcTransferError> {
    use base64::Engine;
    use ed25519_dalek::{Signer, SigningKey};

    let signing_key = SigningKey::from_keypair_bytes(owner_keypair)
        .map_err(|e| UsdcTransferError::InvalidKeypair(e.to_string()))?;
    let signer_pubkey = signing_key.verifying_key().to_bytes();

    if &signer_pubkey != owner {
        return Err(UsdcTransferError::KeypairMismatch);
    }

    let msg = build_usdc_transfer_checked_message(
        owner,
        destination_wallet,
        mint,
        amount,
        recent_blockhash,
    )?;
    let signature = signing_key.sign(&msg);

    // Wire tx: compact-u16(1) || signature(64) || message.
    let mut wire_bytes = Vec::with_capacity(1 + 64 + msg.len());
    wire_bytes.push(0x01);
    wire_bytes.extend_from_slice(&signature.to_bytes());
    wire_bytes.extend_from_slice(&msg);

    Ok(SignedUsdcTransfer {
        base64_tx: base64::engine::general_purpose::STANDARD.encode(&wire_bytes),
        signature_b58: bs58::encode(signature.to_bytes()).into_string(),
        wire_bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solana_types::VersionedTransaction;
    use crate::spl_transfer::extract_spl_transfer;

    /// Fixed test inputs. The owner is the pubkey of the deterministic test
    /// signing key (seed `[1u8; 32]`) so the signing tests can use a valid
    /// keypair for the same message the golden test pins.
    fn test_owner_keypair() -> ([u8; 32], [u8; 64]) {
        use ed25519_dalek::SigningKey;
        let signing_key = SigningKey::from_bytes(&[1u8; 32]);
        let pubkey = signing_key.verifying_key().to_bytes();
        let mut keypair = [0u8; 64];
        keypair[..32].copy_from_slice(&signing_key.to_bytes());
        keypair[32..].copy_from_slice(&pubkey);
        (pubkey, keypair)
    }

    fn test_destination() -> [u8; 32] {
        [0x22u8; 32]
    }

    /// Mainnet USDC mint bytes.
    fn test_mint() -> [u8; 32] {
        crate::escrow::pda::decode_bs58_pubkey("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
            .expect("valid mint")
    }

    const TEST_AMOUNT: u64 = 46_220; // the plan's regression refund figure
    const TEST_BLOCKHASH: [u8; 32] = [7u8; 32];

    /// Golden vector: the expected message is assembled BY HAND here from the
    /// documented specs — the Solana legacy-message framing, the SPL ATA
    /// program's CreateIdempotent (data `[1]`, accounts `[funder, ata, wallet,
    /// mint, system, token]`, verified against
    /// `solana-program/associated-token-account` interface 2026-07-03), and the
    /// SPL Token TransferChecked (data `[12 || u64_le(amount) || decimals]`,
    /// accounts `[source, mint, destination, authority]`) — NOT by calling the
    /// builder, so this test independently re-derives the byte layout. The
    /// ATAs come from [`derive_ata`], which is itself pinned by the escrow
    /// deposit golden vectors.
    #[test]
    fn build_message_matches_hand_assembled_golden_vector() {
        let (owner, _) = test_owner_keypair();
        let destination = test_destination();
        let mint = test_mint();

        let source_ata =
            derive_ata(&Pubkey(owner), &Pubkey(mint), &Pubkey::TOKEN_PROGRAM_ID).unwrap();
        let dest_ata = derive_ata(
            &Pubkey(destination),
            &Pubkey(mint),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .unwrap();

        let mut expected: Vec<u8> = Vec::new();
        // Legacy header: 1 signer, 0 readonly-signed, 5 readonly-unsigned.
        expected.extend_from_slice(&[1, 0, 5]);
        // 8 account keys.
        expected.push(8);
        expected.extend_from_slice(&owner);
        expected.extend_from_slice(&source_ata.0);
        expected.extend_from_slice(&dest_ata.0);
        expected.extend_from_slice(&destination);
        expected.extend_from_slice(&mint);
        expected.extend_from_slice(&[0u8; 32]); // system program
        expected.extend_from_slice(&Pubkey::TOKEN_PROGRAM_ID.0);
        expected.extend_from_slice(&ASSOCIATED_TOKEN_PROGRAM_ID.0);
        // Recent blockhash.
        expected.extend_from_slice(&TEST_BLOCKHASH);
        // Two instructions.
        expected.push(2);
        // CreateIdempotent: program = ata_program (7);
        // accounts [funder=0, ata=2, wallet=3, mint=4, system=5, token=6];
        // data [1].
        expected.extend_from_slice(&[7, 6, 0, 2, 3, 4, 5, 6, 1, 1]);
        // TransferChecked: program = token_program (6);
        // accounts [source=1, mint=4, destination=2, authority=0];
        // data [12 || amount_le(8) || decimals].
        expected.extend_from_slice(&[6, 4, 1, 4, 2, 0, 10, 12]);
        expected.extend_from_slice(&TEST_AMOUNT.to_le_bytes());
        expected.push(6);

        let built = build_usdc_transfer_checked_message(
            &owner,
            &destination,
            &mint,
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
        )
        .expect("builds");

        assert_eq!(
            built, expected,
            "refund message bytes drifted from the hand-assembled golden layout"
        );
    }

    /// Cross-check with the INDEPENDENT in-tree decoder: the verifier-side
    /// transaction parser + SPL transfer extractor (written for inbound
    /// payment verification, long before this builder) must read back exactly
    /// the transfer this builder encodes.
    #[test]
    fn signed_transfer_round_trips_through_the_verifier_parser() {
        let (owner, keypair) = test_owner_keypair();
        let destination = test_destination();
        let mint = test_mint();

        let signed = sign_usdc_transfer_checked(
            &owner,
            &destination,
            &mint,
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
            &keypair,
        )
        .expect("signs");

        let tx = VersionedTransaction::from_bytes(&signed.wire_bytes).expect("parses");
        let msg = tx.parse_message().expect("message parses");

        // The extractor finds the TransferChecked (it skips the ATA-program
        // CreateIdempotent — not a token-program instruction).
        let transfer = extract_spl_transfer(&msg).expect("finds the transfer");
        let expected_dest_ata = derive_ata(
            &Pubkey(destination),
            &Pubkey(mint),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .unwrap();
        assert_eq!(transfer.amount, TEST_AMOUNT);
        assert_eq!(transfer.destination, expected_dest_ata);
        assert_eq!(transfer.mint, Some(Pubkey(mint)));

        // The signature verifies over the message bytes with the owner key.
        use ed25519_dalek::{Verifier, VerifyingKey};
        let sig_bytes: [u8; 64] = bs58::decode(&signed.signature_b58)
            .into_vec()
            .unwrap()
            .try_into()
            .unwrap();
        let vk = VerifyingKey::from_bytes(&owner).unwrap();
        let message_bytes = &signed.wire_bytes[65..];
        vk.verify(
            message_bytes,
            &ed25519_dalek::Signature::from_bytes(&sig_bytes),
        )
        .expect("signature must verify over the message with the owner key");
    }

    #[test]
    fn build_rejects_zero_amount() {
        let (owner, _) = test_owner_keypair();
        assert!(matches!(
            build_usdc_transfer_checked_message(
                &owner,
                &test_destination(),
                &test_mint(),
                0,
                &TEST_BLOCKHASH,
            ),
            Err(UsdcTransferError::ZeroAmount)
        ));
    }

    #[test]
    fn sign_rejects_keypair_owner_mismatch() {
        let (_, keypair) = test_owner_keypair();
        // Declare a DIFFERENT owner than the keypair's pubkey — must refuse to
        // sign (never sign a message whose fee payer is a key we don't hold).
        let wrong_owner = [9u8; 32];
        assert!(matches!(
            sign_usdc_transfer_checked(
                &wrong_owner,
                &test_destination(),
                &test_mint(),
                TEST_AMOUNT,
                &TEST_BLOCKHASH,
                &keypair,
            ),
            Err(UsdcTransferError::KeypairMismatch)
        ));
    }

    #[test]
    fn sign_rejects_inconsistent_keypair() {
        let (owner, mut keypair) = test_owner_keypair();
        // Corrupt the public half — a self-inconsistent keypair must be
        // rejected up front.
        keypair[63] ^= 0xFF;
        assert!(matches!(
            sign_usdc_transfer_checked(
                &owner,
                &test_destination(),
                &test_mint(),
                TEST_AMOUNT,
                &TEST_BLOCKHASH,
                &keypair,
            ),
            Err(UsdcTransferError::InvalidKeypair(_))
        ));
    }

    #[test]
    fn create_idempotent_instruction_bytes_are_pinned() {
        // Addendum item 4: CreateIdempotent has no other in-tree encoding
        // precedent, so its instruction bytes get their own pin. Data is the
        // single Borsh discriminant byte `1`; account order is
        // [funder, ata, wallet, mint, system_program, token_program]
        // (canonical ATA-program interface).
        let (owner, _) = test_owner_keypair();
        let built = build_usdc_transfer_checked_message(
            &owner,
            &test_destination(),
            &test_mint(),
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
        )
        .unwrap();
        let tx_msg = {
            // Parse via the independent decoder to locate instruction 0.
            let mut wire = vec![0u8]; // 0 signatures — parser only needs framing
            wire.extend_from_slice(&built);
            VersionedTransaction::from_bytes(&wire)
                .unwrap()
                .parse_message()
                .unwrap()
        };
        let create = &tx_msg.instructions[0];
        assert_eq!(
            tx_msg.account_keys[create.program_id_index as usize], ASSOCIATED_TOKEN_PROGRAM_ID,
            "instruction 0 must target the ATA program"
        );
        assert_eq!(create.data, vec![1u8], "CreateIdempotent data byte");
        // Resolve the instruction's account indices back to keys and check
        // the canonical order.
        let keys: Vec<Pubkey> = create
            .accounts
            .iter()
            .map(|&i| tx_msg.account_keys[i as usize])
            .collect();
        let dest_ata = derive_ata(
            &Pubkey(test_destination()),
            &Pubkey(test_mint()),
            &Pubkey::TOKEN_PROGRAM_ID,
        )
        .unwrap();
        assert_eq!(keys[0], Pubkey(owner), "funder");
        assert_eq!(keys[1], dest_ata, "associated token account");
        assert_eq!(keys[2], Pubkey(test_destination()), "wallet");
        assert_eq!(keys[3], Pubkey(test_mint()), "mint");
        assert_eq!(keys[4], Pubkey::SYSTEM_PROGRAM, "system program");
        assert_eq!(keys[5], Pubkey::TOKEN_PROGRAM_ID, "token program");
    }
}
