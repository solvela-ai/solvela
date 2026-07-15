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
//! The `_with_memo` variants (#743) append an SPL Memo instruction carrying a
//! per-obligation id so two distinct refund obligations can NEVER serialize to
//! byte-identical transactions (identical bytes = one signature = cluster-level
//! dedupe into a single landed transfer, with every sibling row
//! phantom-confirming off the shared signature's status). The dual
//! [`verify_signed_usdc_transfer`] is the confirm gate: it proves persisted
//! bytes are the landed transaction for THIS obligation before any row is
//! stamped confirmed.
//!
//! Money-path rules (solvela-x402 §6): integer atomic amounts only, zero
//! amounts rejected, the signer keypair validated against the declared source
//! owner (never sign for a mismatched fee payer), key bytes zeroized by the
//! caller (the gateway holds them in a [`crate::fee_payer::FeePayerWallet`]).

use crate::solana_types::{
    derive_ata, Pubkey, TransactionError, VersionedTransaction, ASSOCIATED_TOKEN_PROGRAM_ID,
    MEMO_PROGRAM_ID,
};
use crate::spl_transfer::extract_spl_transfer;

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

/// Longest memo the single-byte compact-u16 data-length prefix can carry.
/// Refund memos are `solvela-refund:<base58 channel id>` (~60 bytes); anything
/// larger is a corrupt input to reject, never a layout to support.
const MAX_MEMO_LEN: usize = 127;

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
    /// A memo was requested but empty — an empty memo cannot uniquify the
    /// transaction (#743), so it is refused, never silently omitted.
    #[error("memo must not be empty")]
    EmptyMemo,
    /// The memo exceeds the single-byte compact-u16 data-length prefix.
    #[error("memo of {len} bytes exceeds the {MAX_MEMO_LEN}-byte limit")]
    MemoTooLong { len: usize },
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
pub(crate) fn build_usdc_transfer_checked_message(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
) -> Result<Vec<u8>, UsdcTransferError> {
    build_transfer_message(
        owner,
        destination_wallet,
        mint,
        amount,
        recent_blockhash,
        None,
    )
}

/// [`build_usdc_transfer_checked_message`] plus an SPL Memo instruction
/// carrying `memo`, so the transaction bytes are unique per obligation.
///
/// **Required for channel refunds (#743):** two refunds with an equal
/// `(source, destination, mint, amount)` tuple signed against the same recent
/// blockhash are byte-identical without a memo — one signature, which the
/// cluster dedupes into a SINGLE landed transfer while every sibling refund
/// row phantom-confirms off that signature's status.
///
/// Layout delta from the memoless form: the memo program is appended as
/// account key 8 (readonly tail; header becomes `[1, 0, 6]`) and instruction 3
/// is `Memo` (program index 8): zero accounts, data = the memo's UTF-8 bytes
/// (the program validates UTF-8 on-chain; `&str` input guarantees it here).
/// The first two instructions are bit-identical to the memoless layout.
pub(crate) fn build_usdc_transfer_checked_with_memo_message(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    memo: &str,
) -> Result<Vec<u8>, UsdcTransferError> {
    build_transfer_message(
        owner,
        destination_wallet,
        mint,
        amount,
        recent_blockhash,
        Some(memo),
    )
}

/// Shared core for the memoless and with-memo layouts. `memo: None` produces
/// bytes identical to the pre-#743 builder (golden-vector-pinned above).
fn build_transfer_message(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    memo: Option<&str>,
) -> Result<Vec<u8>, UsdcTransferError> {
    if amount == 0 {
        return Err(UsdcTransferError::ZeroAmount);
    }
    if let Some(m) = memo {
        if m.is_empty() {
            return Err(UsdcTransferError::EmptyMemo);
        }
        if m.len() > MAX_MEMO_LEN {
            return Err(UsdcTransferError::MemoTooLong { len: m.len() });
        }
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

    // Header: 1 required signature, 0 readonly-signed, 5 readonly-unsigned
    // (6 with the memo program appended to the readonly tail).
    msg.extend_from_slice(&[1u8, 0u8, if memo.is_some() { 6u8 } else { 5u8 }]);

    // Account keys (8, or 9 with memo). Single-byte compact-u16 count.
    msg.push(if memo.is_some() { 9u8 } else { 8u8 });
    msg.extend_from_slice(owner);
    msg.extend_from_slice(&source_ata.0);
    msg.extend_from_slice(&destination_ata.0);
    msg.extend_from_slice(destination_wallet);
    msg.extend_from_slice(mint);
    msg.extend_from_slice(&system_program);
    msg.extend_from_slice(&Pubkey::TOKEN_PROGRAM_ID.0);
    msg.extend_from_slice(&ASSOCIATED_TOKEN_PROGRAM_ID.0);
    if memo.is_some() {
        msg.extend_from_slice(&MEMO_PROGRAM_ID.0);
    }

    // Recent blockhash.
    msg.extend_from_slice(recent_blockhash);

    // Instruction count: 2 (3 with memo).
    msg.push(if memo.is_some() { 3u8 } else { 2u8 });

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

    // Instruction 3 (with memo only): Memo — zero accounts, UTF-8 data.
    if let Some(m) = memo {
        msg.push(8u8); // program_id_index = memo_program
        msg.push(0u8); // account index count
        msg.push(m.len() as u8); // data length (<= MAX_MEMO_LEN, checked above)
        msg.extend_from_slice(m.as_bytes());
    }

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
pub(crate) fn sign_usdc_transfer_checked(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    owner_keypair: &[u8; 64],
) -> Result<SignedUsdcTransfer, UsdcTransferError> {
    sign_transfer_message(
        owner,
        destination_wallet,
        mint,
        amount,
        recent_blockhash,
        None,
        owner_keypair,
    )
}

/// [`sign_usdc_transfer_checked`] over the with-memo layout — see
/// [`build_usdc_transfer_checked_with_memo_message`]. The signing surface the
/// channel-refund worker MUST use (#743): memoless equal obligations collide
/// into one signature and get deduped on-chain.
pub(crate) fn sign_usdc_transfer_checked_with_memo(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    memo: &str,
    owner_keypair: &[u8; 64],
) -> Result<SignedUsdcTransfer, UsdcTransferError> {
    sign_transfer_message(
        owner,
        destination_wallet,
        mint,
        amount,
        recent_blockhash,
        Some(memo),
        owner_keypair,
    )
}

/// Shared signing core (keypair validation + owner match + assembly).
fn sign_transfer_message(
    owner: &[u8; 32],
    destination_wallet: &[u8; 32],
    mint: &[u8; 32],
    amount: u64,
    recent_blockhash: &[u8; 32],
    memo: Option<&str>,
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

    // Route through the NAMED builders (not the core) so the golden-vector
    // tests pin exactly the functions production signs with.
    let msg = match memo {
        Some(m) => build_usdc_transfer_checked_with_memo_message(
            owner,
            destination_wallet,
            mint,
            amount,
            recent_blockhash,
            m,
        )?,
        None => build_usdc_transfer_checked_message(
            owner,
            destination_wallet,
            mint,
            amount,
            recent_blockhash,
        )?,
    };
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

// ---------------------------------------------------------------------------
// Landed-transaction content verification (#743 confirm gate)
// ---------------------------------------------------------------------------

/// The exact transfer a landed refund transaction must contain before its
/// `channel_refunds` row may be stamped `confirmed`.
#[derive(Debug)]
pub struct ExpectedUsdcTransfer<'a> {
    /// Base58 transaction signature whose on-chain status was checked — the
    /// key that binds the persisted bytes to the landed transaction.
    pub signature_b58: &'a str,
    /// The transfer source owner (fee payer + token authority): the pubkey
    /// the embedded signature must cryptographically verify under.
    pub source_owner: &'a [u8; 32],
    /// The obligation's frozen destination wallet (NOT a raw token account).
    pub destination_wallet: &'a [u8; 32],
    /// The obligation's frozen mint.
    pub mint: &'a [u8; 32],
    /// The obligation's frozen atomic amount.
    pub amount: u64,
    /// The per-obligation memo (`solvela-refund:<channel_id>` for refunds).
    pub memo: &'a str,
}

/// Errors from [`verify_signed_usdc_transfer`]. Every variant means the
/// caller MUST NOT confirm the obligation (fail closed).
#[derive(Debug, thiserror::Error)]
pub enum UsdcTransferVerifyError {
    /// The persisted bytes do not parse as a Solana transaction.
    #[error("transaction bytes do not parse: {0}")]
    Unparsable(#[from] TransactionError),
    /// The transaction does not carry exactly one signature.
    #[error("expected exactly one signature, found {0}")]
    SignatureCount(usize),
    /// The embedded signature is not the tracked transaction signature — the
    /// bytes describe a DIFFERENT transaction than the one whose status was
    /// checked.
    #[error("embedded signature does not match the tracked transaction signature")]
    SignatureMismatch,
    /// The expected source owner is not a valid ed25519 public key.
    #[error("source owner is not a valid ed25519 public key")]
    InvalidOwnerKey,
    /// The signature does not verify over the message under the source owner
    /// — the message bytes were altered after signing.
    #[error("signature does not verify over the message under the source owner")]
    SignatureInvalid,
    /// No SPL transfer instruction was found in the message.
    #[error("no SPL transfer instruction found: {0}")]
    MissingTransfer(String),
    /// The transfer amount is not the obligation amount.
    #[error("transfer amount {found} does not match the obligation amount {expected}")]
    AmountMismatch { expected: u64, found: u64 },
    /// The expected destination ATA could not be derived.
    #[error("could not derive the destination associated token account")]
    AtaDerivation,
    /// The transfer destination is not the obligation destination's ATA.
    #[error("transfer destination is not the obligation destination ATA")]
    DestinationMismatch,
    /// The transfer mint is not the obligation mint (or the instruction was a
    /// mint-less plain `Transfer`).
    #[error("transfer mint does not match the obligation mint")]
    MintMismatch,
    /// The transaction does not carry exactly one memo instruction with
    /// exactly this obligation's memo bytes.
    #[error("memo does not identify this obligation")]
    MemoMismatch,
}

/// Verify that signed transfer bytes are the landed transaction for THIS
/// obligation — the #743 confirm gate. A `getSignatureStatuses` hit alone is
/// NOT proof the obligation was paid: byte-identical sibling transfers share
/// one signature, the cluster dedupes them into one landed transaction, and
/// every sibling then "confirms" while only one payment moved.
///
/// Entirely local, no `getTransaction` round-trip: the cluster confirmed
/// `signature_b58`; an ed25519 signature commits to its message bytes; so a
/// transaction whose (a) sole embedded signature IS the tracked signature,
/// (b) signature verifies over the message under `source_owner`, and
/// (c) message contains this obligation's memo and frozen
/// `(destination, mint, amount)` transfer, IS the landed transaction paying
/// this obligation. Any failure means the caller must NOT confirm.
pub fn verify_signed_usdc_transfer(
    wire_bytes: &[u8],
    expected: &ExpectedUsdcTransfer<'_>,
) -> Result<(), UsdcTransferVerifyError> {
    use ed25519_dalek::{Signature, Verifier, VerifyingKey};

    let tx = VersionedTransaction::from_bytes(wire_bytes)?;
    if tx.signatures.len() != 1 {
        return Err(UsdcTransferVerifyError::SignatureCount(tx.signatures.len()));
    }
    let sig_bytes = tx.signatures[0].0;
    if bs58::encode(sig_bytes).into_string() != expected.signature_b58 {
        return Err(UsdcTransferVerifyError::SignatureMismatch);
    }

    let verifying_key = VerifyingKey::from_bytes(expected.source_owner)
        .map_err(|_| UsdcTransferVerifyError::InvalidOwnerKey)?;
    verifying_key
        .verify(&tx.message_bytes, &Signature::from_bytes(&sig_bytes))
        .map_err(|_| UsdcTransferVerifyError::SignatureInvalid)?;

    let msg = tx.parse_message()?;

    let transfer = extract_spl_transfer(&msg)
        .map_err(|e| UsdcTransferVerifyError::MissingTransfer(e.to_string()))?;
    if transfer.amount != expected.amount {
        return Err(UsdcTransferVerifyError::AmountMismatch {
            expected: expected.amount,
            found: transfer.amount,
        });
    }
    let destination_ata = derive_ata(
        &Pubkey(*expected.destination_wallet),
        &Pubkey(*expected.mint),
        &Pubkey::TOKEN_PROGRAM_ID,
    )
    .ok_or(UsdcTransferVerifyError::AtaDerivation)?;
    if transfer.destination != destination_ata {
        return Err(UsdcTransferVerifyError::DestinationMismatch);
    }
    if transfer.mint != Some(Pubkey(*expected.mint)) {
        return Err(UsdcTransferVerifyError::MintMismatch);
    }

    // Exactly ONE memo instruction, carrying exactly this obligation's bytes.
    let mut memos = msg
        .instructions
        .iter()
        .filter(|ix| msg.account_keys.get(ix.program_id_index as usize) == Some(&MEMO_PROGRAM_ID));
    match (memos.next(), memos.next()) {
        (Some(memo_ix), None) if memo_ix.data == expected.memo.as_bytes() => Ok(()),
        _ => Err(UsdcTransferVerifyError::MemoMismatch),
    }
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

    // -- #743: per-obligation memo uniqueness + landed-tx content verification --

    const TEST_MEMO: &str = "solvela-refund:test-channel-1";

    fn signed_with_memo(memo: &str) -> SignedUsdcTransfer {
        let (owner, keypair) = test_owner_keypair();
        sign_usdc_transfer_checked_with_memo(
            &owner,
            &test_destination(),
            &test_mint(),
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
            memo,
            &keypair,
        )
        .expect("signs")
    }

    fn expected_transfer<'a>(
        signature_b58: &'a str,
        owner: &'a [u8; 32],
        destination: &'a [u8; 32],
        mint: &'a [u8; 32],
        memo: &'a str,
    ) -> ExpectedUsdcTransfer<'a> {
        ExpectedUsdcTransfer {
            signature_b58,
            source_owner: owner,
            destination_wallet: destination,
            mint,
            amount: TEST_AMOUNT,
            memo,
        }
    }

    /// The #743 root cause AND its fix in one pin. Root cause: two equal
    /// refund obligations signed against one blockhash WITHOUT a memo are
    /// byte-identical — one signature, which the cluster dedupes into a
    /// single landed transfer while every sibling row phantom-confirms.
    /// Fix: distinct per-obligation memos make collision impossible.
    #[test]
    fn distinct_memos_make_equal_obligations_unique() {
        let (owner, keypair) = test_owner_keypair();
        let memoless = |_: ()| {
            sign_usdc_transfer_checked(
                &owner,
                &test_destination(),
                &test_mint(),
                TEST_AMOUNT,
                &TEST_BLOCKHASH,
                &keypair,
            )
            .expect("signs")
        };
        let a = memoless(());
        let b = memoless(());
        assert_eq!(
            a.wire_bytes, b.wire_bytes,
            "memoless equal obligations collide — the #743 dedupe precondition"
        );
        assert_eq!(a.signature_b58, b.signature_b58);

        let a = signed_with_memo("solvela-refund:channel-a");
        let b = signed_with_memo("solvela-refund:channel-b");
        assert_ne!(
            a.wire_bytes, b.wire_bytes,
            "per-obligation memos must make the transaction bytes unique"
        );
        assert_ne!(
            a.signature_b58, b.signature_b58,
            "distinct bytes must yield distinct signatures (the ledger dedupe key)"
        );
    }

    /// Golden vector for the with-memo layout, hand-assembled from the specs
    /// (never from the builder's own output): the memoless framing above plus
    /// the SPL Memo program appended as account 8 (header `[1, 0, 6]`) and a
    /// third instruction — program 8, zero accounts, data = the memo's UTF-8
    /// bytes. The memoless golden vector is UNTOUCHED: that layout still
    /// builds byte-for-byte identically.
    #[test]
    fn build_with_memo_matches_hand_assembled_golden_vector() {
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
        // Legacy header: 1 signer, 0 readonly-signed, 6 readonly-unsigned.
        expected.extend_from_slice(&[1, 0, 6]);
        // 9 account keys (memoless 8 + the memo program in the readonly tail).
        expected.push(9);
        expected.extend_from_slice(&owner);
        expected.extend_from_slice(&source_ata.0);
        expected.extend_from_slice(&dest_ata.0);
        expected.extend_from_slice(&destination);
        expected.extend_from_slice(&mint);
        expected.extend_from_slice(&[0u8; 32]); // system program
        expected.extend_from_slice(&Pubkey::TOKEN_PROGRAM_ID.0);
        expected.extend_from_slice(&ASSOCIATED_TOKEN_PROGRAM_ID.0);
        expected.extend_from_slice(&MEMO_PROGRAM_ID.0);
        expected.extend_from_slice(&TEST_BLOCKHASH);
        // Three instructions; the first two are bit-identical to the memoless
        // layout (account indices unchanged — memo appended after them).
        expected.push(3);
        expected.extend_from_slice(&[7, 6, 0, 2, 3, 4, 5, 6, 1, 1]);
        expected.extend_from_slice(&[6, 4, 1, 4, 2, 0, 10, 12]);
        expected.extend_from_slice(&TEST_AMOUNT.to_le_bytes());
        expected.push(6);
        // Memo: program = memo_program (8), zero accounts, data = UTF-8 memo.
        expected.push(8);
        expected.push(0);
        expected.push(TEST_MEMO.len() as u8);
        expected.extend_from_slice(TEST_MEMO.as_bytes());

        let built = build_usdc_transfer_checked_with_memo_message(
            &owner,
            &destination,
            &mint,
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
            TEST_MEMO,
        )
        .expect("builds");

        assert_eq!(
            built, expected,
            "with-memo refund message bytes drifted from the hand-assembled golden layout"
        );
    }

    #[test]
    fn build_with_memo_rejects_empty_memo() {
        let (owner, _) = test_owner_keypair();
        assert!(matches!(
            build_usdc_transfer_checked_with_memo_message(
                &owner,
                &test_destination(),
                &test_mint(),
                TEST_AMOUNT,
                &TEST_BLOCKHASH,
                "",
            ),
            Err(UsdcTransferError::EmptyMemo)
        ));
    }

    #[test]
    fn build_with_memo_rejects_oversized_memo() {
        // 128 bytes exceeds the single-byte compact-u16 data-length prefix —
        // reject, never truncate.
        let (owner, _) = test_owner_keypair();
        let oversized = "m".repeat(128);
        assert!(matches!(
            build_usdc_transfer_checked_with_memo_message(
                &owner,
                &test_destination(),
                &test_mint(),
                TEST_AMOUNT,
                &TEST_BLOCKHASH,
                &oversized,
            ),
            Err(UsdcTransferError::MemoTooLong { len: 128 })
        ));
    }

    #[test]
    fn memo_program_id_matches_canonical_base58() {
        assert_eq!(
            MEMO_PROGRAM_ID.to_string(),
            "MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr",
            "SPL Memo v2 program id (solana-program.com/docs/memo, 2026-07-14)"
        );
    }

    #[test]
    fn verify_accepts_the_signed_transfer() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        verify_signed_usdc_transfer(
            &signed.wire_bytes,
            &expected_transfer(
                &signed.signature_b58,
                &owner,
                &test_destination(),
                &test_mint(),
                TEST_MEMO,
            ),
        )
        .expect("the builder's own output must verify");
    }

    /// The confirm-gate half of #743: a landed transaction carrying a SIBLING
    /// obligation's memo must never verify as THIS obligation.
    #[test]
    fn verify_rejects_a_sibling_obligations_memo() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo("solvela-refund:sibling-channel");
        assert!(matches!(
            verify_signed_usdc_transfer(
                &signed.wire_bytes,
                &expected_transfer(
                    &signed.signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::MemoMismatch)
        ));
    }

    /// The exact incident shape: pre-#743 memoless bytes. Without a memo the
    /// landed transfer cannot be proven to pay THIS obligation — refuse.
    #[test]
    fn verify_rejects_memoless_bytes() {
        let (owner, keypair) = test_owner_keypair();
        let signed = sign_usdc_transfer_checked(
            &owner,
            &test_destination(),
            &test_mint(),
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
            &keypair,
        )
        .expect("signs");
        assert!(matches!(
            verify_signed_usdc_transfer(
                &signed.wire_bytes,
                &expected_transfer(
                    &signed.signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::MemoMismatch)
        ));
    }

    #[test]
    fn verify_rejects_wrong_amount() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        let destination = test_destination();
        let mint = test_mint();
        let mut expected = expected_transfer(
            &signed.signature_b58,
            &owner,
            &destination,
            &mint,
            TEST_MEMO,
        );
        expected.amount = TEST_AMOUNT + 1;
        assert!(matches!(
            verify_signed_usdc_transfer(&signed.wire_bytes, &expected),
            Err(UsdcTransferVerifyError::AmountMismatch {
                expected: e,
                found: f
            }) if e == TEST_AMOUNT + 1 && f == TEST_AMOUNT
        ));
    }

    #[test]
    fn verify_rejects_wrong_destination() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        let other_destination = [0x33u8; 32];
        assert!(matches!(
            verify_signed_usdc_transfer(
                &signed.wire_bytes,
                &expected_transfer(
                    &signed.signature_b58,
                    &owner,
                    &other_destination,
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::DestinationMismatch)
        ));
    }

    /// The tracked signature and the persisted bytes must agree — a
    /// cross-contaminated row (bytes from one obligation, signature from
    /// another) can never stamp.
    #[test]
    fn verify_rejects_foreign_signature() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        let foreign = signed_with_memo("solvela-refund:other");
        assert!(matches!(
            verify_signed_usdc_transfer(
                &signed.wire_bytes,
                &expected_transfer(
                    &foreign.signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::SignatureMismatch)
        ));
    }

    /// Corrupted persisted message bytes (signature left intact) must fail
    /// the ed25519 check — the signature is what binds the persisted bytes to
    /// the landed transaction.
    #[test]
    fn verify_rejects_tampered_message_bytes() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        let mut tampered = signed.wire_bytes.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF; // flip a byte inside the message portion
        assert!(matches!(
            verify_signed_usdc_transfer(
                &tampered,
                &expected_transfer(
                    &signed.signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::SignatureInvalid)
        ));
    }

    /// A multi-signature framing can never be the worker's own single-signer
    /// transfer — reject on count before any content is trusted.
    #[test]
    fn verify_rejects_multi_signature_transaction() {
        let (owner, _) = test_owner_keypair();
        let signed = signed_with_memo(TEST_MEMO);
        // Re-frame the wire bytes as a 2-signature transaction.
        let mut wire = vec![2u8];
        wire.extend_from_slice(&signed.wire_bytes[1..65]);
        wire.extend_from_slice(&signed.wire_bytes[1..65]);
        wire.extend_from_slice(&signed.wire_bytes[65..]);
        assert!(matches!(
            verify_signed_usdc_transfer(
                &wire,
                &expected_transfer(
                    &signed.signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::SignatureCount(2))
        ));
    }

    /// A properly-signed message whose TransferChecked mint account is NOT
    /// the obligation mint must fail on the mint check specifically. The
    /// mint account key (index 4) is corrupted while the already-derived
    /// destination-ATA bytes stay untouched, then the message is RE-SIGNED so
    /// no earlier check can mask the mint comparison.
    #[test]
    fn verify_rejects_wrong_mint_account() {
        use ed25519_dalek::{Signer, SigningKey};
        let (owner, keypair) = test_owner_keypair();
        let mut msg = build_usdc_transfer_checked_with_memo_message(
            &owner,
            &test_destination(),
            &test_mint(),
            TEST_AMOUNT,
            &TEST_BLOCKHASH,
            TEST_MEMO,
        )
        .expect("builds");
        // Account key 4 (the mint) starts at 4 + 4*32: header(3) + count(1).
        let mint_offset = 4 + 4 * 32;
        msg[mint_offset] ^= 0xFF;
        let signing_key = SigningKey::from_keypair_bytes(&keypair).expect("valid keypair");
        let signature = signing_key.sign(&msg);
        let mut wire = vec![1u8];
        wire.extend_from_slice(&signature.to_bytes());
        wire.extend_from_slice(&msg);
        let signature_b58 = bs58::encode(signature.to_bytes()).into_string();
        assert!(matches!(
            verify_signed_usdc_transfer(
                &wire,
                &expected_transfer(
                    &signature_b58,
                    &owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::MintMismatch)
        ));
    }

    /// An expected source owner that is not a valid ed25519 encoding must be
    /// rejected up front, never treated as "verification unavailable".
    #[test]
    fn verify_rejects_invalid_owner_key() {
        let signed = signed_with_memo(TEST_MEMO);
        // Deterministically pick a y-encoding that fails point decompression
        // (about half of all field elements do; the first hit is stable).
        let invalid_owner = (0u8..=255)
            .map(|b| {
                let mut k = [0u8; 32];
                k[0] = b;
                k
            })
            .find(|k| ed25519_dalek::VerifyingKey::from_bytes(k).is_err())
            .expect("some single-byte y must not be on the curve");
        assert!(matches!(
            verify_signed_usdc_transfer(
                &signed.wire_bytes,
                &expected_transfer(
                    &signed.signature_b58,
                    &invalid_owner,
                    &test_destination(),
                    &test_mint(),
                    TEST_MEMO,
                ),
            ),
            Err(UsdcTransferVerifyError::InvalidOwnerKey)
        ));
    }

    #[test]
    fn verify_rejects_garbage_bytes() {
        let (owner, _) = test_owner_keypair();
        assert!(matches!(
            verify_signed_usdc_transfer(
                &[0xAB; 7],
                &expected_transfer("sig", &owner, &test_destination(), &test_mint(), TEST_MEMO,),
            ),
            Err(UsdcTransferVerifyError::Unparsable(_))
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
