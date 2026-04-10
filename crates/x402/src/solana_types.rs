//! Minimal Solana types for x402 payment verification.
//!
//! These are lightweight replacements for `solana-sdk` types to avoid its
//! heavy dependency chain (openssl-sys, etc.). When the build environment
//! has OpenSSL or solana-sdk stabilizes without the openssl dep, we can
//! swap these back to the real types.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Pubkey
// ---------------------------------------------------------------------------

/// A Solana public key (32 bytes), base58-encoded for display.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Pubkey(pub [u8; 32]);

impl Pubkey {
    /// The system program address (all zeros).
    pub const SYSTEM_PROGRAM: Pubkey = Pubkey([0u8; 32]);

    /// SPL Token program ID.
    pub const TOKEN_PROGRAM_ID: Pubkey = Pubkey([
        6, 221, 246, 225, 215, 101, 161, 147, 217, 203, 225, 70, 206, 235, 121, 172, 28, 180, 133,
        237, 95, 91, 55, 145, 58, 140, 245, 133, 126, 255, 0, 169,
    ]);

    /// SPL Token-2022 program ID.
    pub const TOKEN_2022_PROGRAM_ID: Pubkey = Pubkey([
        6, 221, 246, 225, 238, 117, 143, 222, 24, 66, 93, 188, 228, 108, 205, 218, 182, 26, 252,
        77, 131, 185, 13, 39, 254, 189, 249, 40, 216, 161, 139, 252,
    ]);
}

impl fmt::Display for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0).into_string())
    }
}

impl fmt::Debug for Pubkey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Pubkey({})", self)
    }
}

/// Error type for Pubkey parsing.
#[derive(Debug, thiserror::Error)]
pub enum PubkeyError {
    #[error("invalid base58: {0}")]
    InvalidBase58(String),

    #[error("invalid length: expected 32 bytes, got {0}")]
    InvalidLength(usize),
}

impl FromStr for Pubkey {
    type Err = PubkeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = bs58::decode(s)
            .into_vec()
            .map_err(|e| PubkeyError::InvalidBase58(e.to_string()))?;

        if bytes.len() != 32 {
            return Err(PubkeyError::InvalidLength(bytes.len()));
        }

        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(Pubkey(arr))
    }
}

impl Serialize for Pubkey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Pubkey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Pubkey::from_str(&s).map_err(serde::de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Associated Token Account (ATA) address derivation
// ---------------------------------------------------------------------------

/// The SPL Associated Token Account program ID.
pub const ASSOCIATED_TOKEN_PROGRAM_ID: Pubkey = Pubkey([
    140, 151, 37, 143, 78, 36, 137, 241, 187, 61, 16, 41, 20, 142, 13, 131, 11, 90, 19, 153, 218,
    255, 16, 132, 4, 142, 123, 216, 219, 233, 248, 89,
]);

/// Derive the Associated Token Account (ATA) address for a given wallet and mint.
///
/// The ATA address is a Program Derived Address (PDA) computed as:
/// `find_program_address([wallet, token_program_id, mint], associated_token_program_id)`
///
/// This is the canonical token account that wallets use to hold SPL tokens.
/// When verifying a payment, the `destination` in the SPL transfer instruction
/// must be the gateway's ATA for the USDC mint — not the raw wallet pubkey.
///
/// Matches the Solana runtime's `create_program_address` + nonce search exactly.
/// Returns `None` if no valid off-curve point is found (does not happen in practice).
pub fn derive_ata(wallet: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Option<Pubkey> {
    use sha2::{Digest, Sha256};

    // Seeds: [wallet_pubkey_bytes, token_program_id_bytes, mint_pubkey_bytes]
    let seeds: &[&[u8]] = &[&wallet.0, &token_program.0, &mint.0];
    let program_id = &ASSOCIATED_TOKEN_PROGRAM_ID;

    // find_program_address: iterate nonce 255 down to 0, return first off-curve hash
    for nonce in (0u8..=255).rev() {
        // Hash: SHA256(seed0 || seed1 || seed2 || nonce || program_id || "ProgramDerivedAddress")
        // Matches `solana_program::pubkey::Pubkey::create_program_address` exactly —
        // the program id comes BEFORE the PDA marker.
        let mut hasher = Sha256::new();
        for seed in seeds {
            hasher.update(seed);
        }
        hasher.update([nonce]);
        hasher.update(program_id.0);
        hasher.update(b"ProgramDerivedAddress");
        let hash = hasher.finalize();

        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&hash);

        // PDAs must be off the ed25519 curve. The Solana runtime rejects on-curve
        // points because they would be valid public keys (controlled by someone).
        // We use the same check: if the bytes form a valid compressed ed25519 point
        // (y-coordinate with a valid x), skip it. Otherwise, accept it as the PDA.
        if !is_on_ed25519_curve(&bytes) {
            return Some(Pubkey(bytes));
        }
    }

    None
}

/// Check if 32 bytes represent a valid compressed point on the ed25519 curve.
///
/// Uses `curve25519-dalek`'s `CompressedEdwardsY::decompress()` — the same
/// check the Solana runtime performs. A point is "on curve" if and only if
/// decompression succeeds (i.e. the encoded y-coordinate yields a valid x).
///
/// PDAs must be *off* the curve; this function returning `false` means the
/// candidate hash byte array is a valid PDA address.
fn is_on_ed25519_curve(bytes: &[u8; 32]) -> bool {
    use curve25519_dalek::edwards::CompressedEdwardsY;
    CompressedEdwardsY(*bytes).decompress().is_some()
}

// ---------------------------------------------------------------------------
// Signature
// ---------------------------------------------------------------------------

/// A Solana transaction signature (64 bytes).
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Signature(pub [u8; 64]);

impl fmt::Display for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", bs58::encode(&self.0).into_string())
    }
}

impl fmt::Debug for Signature {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Sig({})", self)
    }
}

// ---------------------------------------------------------------------------
// Compact-u16 decoder (Solana wire format)
// ---------------------------------------------------------------------------

/// Decode a compact-u16 value from the Solana wire format.
///
/// Solana uses a variable-length encoding for array lengths in serialized
/// transactions (via `#[serde(with = "short_vec")]`). Each byte contributes
/// 7 bits; the high bit indicates continuation. Maximum 3 bytes (max value 0x7FFF).
///
/// Returns `(decoded_value, bytes_consumed)`.
pub fn decode_compact_u16(data: &[u8], offset: usize) -> Result<(usize, usize), TransactionError> {
    if offset >= data.len() {
        return Err(TransactionError::TooShort);
    }

    let mut value: usize = 0;
    let mut consumed: usize = 0;

    for i in 0..3 {
        let idx = offset + i;
        if idx >= data.len() {
            return Err(TransactionError::TooShort);
        }
        let byte = data[idx] as usize;
        consumed += 1;
        value |= (byte & 0x7F) << (i * 7);
        if byte & 0x80 == 0 {
            return Ok((value, consumed));
        }
    }

    // If we consumed 3 bytes and the last one still had the continuation bit,
    // the encoding is invalid for compact-u16.
    Err(TransactionError::InvalidFormat(
        "compact-u16 overflow: more than 3 bytes".to_string(),
    ))
}

// ---------------------------------------------------------------------------
// VersionedTransaction (minimal stub)
// ---------------------------------------------------------------------------

/// A minimal representation of a Solana versioned transaction.
///
/// This is a lightweight struct for deserializing bincode-encoded transactions
/// from the wire. It captures the signatures and the raw message bytes for
/// introspection.
///
/// The real `solana_sdk::transaction::VersionedTransaction` uses compact-u16
/// (`#[serde(with = "short_vec")]`) for the signature count, even in bincode.
#[derive(Debug, Clone)]
pub struct VersionedTransaction {
    /// Transaction signatures.
    pub signatures: Vec<Signature>,
    /// Raw message bytes (parsed on demand via `parse_message()`).
    pub message_bytes: Vec<u8>,
}

impl VersionedTransaction {
    /// Decode a versioned transaction from bincode bytes.
    ///
    /// Wire layout:
    ///   - compact-u16: number of signatures
    ///   - N * 64 bytes: signatures
    ///   - remaining: versioned message bytes
    pub fn from_bytes(data: &[u8]) -> Result<Self, TransactionError> {
        if data.is_empty() {
            return Err(TransactionError::TooShort);
        }

        // Read signature count as compact-u16
        let (sig_count, sig_count_len) = decode_compact_u16(data, 0)?;

        // Sanity check: a valid Solana tx has at most ~127 signatures
        if sig_count > 127 {
            return Err(TransactionError::InvalidFormat(format!(
                "too many signatures: {sig_count}"
            )));
        }

        let sigs_start = sig_count_len;
        let sigs_end = sigs_start + sig_count * 64;
        if data.len() < sigs_end {
            return Err(TransactionError::TooShort);
        }

        let mut signatures = Vec::with_capacity(sig_count);
        for i in 0..sig_count {
            let offset = sigs_start + i * 64;
            let mut sig = [0u8; 64];
            sig.copy_from_slice(&data[offset..offset + 64]);
            signatures.push(Signature(sig));
        }

        let message_bytes = data[sigs_end..].to_vec();

        Ok(Self {
            signatures,
            message_bytes,
        })
    }

    /// Parse the message bytes into a structured `ParsedMessage`.
    pub fn parse_message(&self) -> Result<ParsedMessage, TransactionError> {
        ParsedMessage::from_bytes(&self.message_bytes)
    }
}

/// Errors from transaction deserialization.
#[derive(Debug, thiserror::Error)]
pub enum TransactionError {
    #[error("transaction data too short")]
    TooShort,

    #[error("invalid transaction format: {0}")]
    InvalidFormat(String),
}

// ---------------------------------------------------------------------------
// Message header
// ---------------------------------------------------------------------------

/// The header of a Solana transaction message.
#[derive(Debug, Clone)]
pub struct MessageHeader {
    pub num_required_signatures: u8,
    pub num_readonly_signed_accounts: u8,
    pub num_readonly_unsigned_accounts: u8,
}

// ---------------------------------------------------------------------------
// Compiled instruction
// ---------------------------------------------------------------------------

/// A compiled instruction within a Solana transaction message.
#[derive(Debug, Clone)]
pub struct CompiledInstruction {
    /// Index into the account keys array for the program being invoked.
    pub program_id_index: u8,
    /// Account indices referenced by this instruction.
    pub accounts: Vec<u8>,
    /// Instruction data.
    pub data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Parsed message
// ---------------------------------------------------------------------------

/// A parsed Solana transaction message (legacy or V0).
#[derive(Debug, Clone)]
pub struct ParsedMessage {
    /// Message header.
    pub header: MessageHeader,
    /// All account public keys in the message.
    pub account_keys: Vec<Pubkey>,
    /// Recent blockhash (32 bytes).
    pub recent_blockhash: [u8; 32],
    /// Compiled instructions.
    pub instructions: Vec<CompiledInstruction>,
    /// Whether this is a V0 message.
    pub is_v0: bool,
}

impl ParsedMessage {
    /// Parse a Solana message from its wire-format bytes.
    ///
    /// Legacy message (first byte < 0x80):
    ///   header (3 bytes) + compact-u16 account_keys + 32-byte blockhash
    ///   + compact-u16 instructions
    ///
    /// V0 message (first byte == 0x80):
    ///   skip version prefix byte, then same layout as legacy
    ///   (address table lookups at end are skipped)
    pub fn from_bytes(data: &[u8]) -> Result<Self, TransactionError> {
        if data.is_empty() {
            return Err(TransactionError::TooShort);
        }

        let mut offset = 0;
        let is_v0 = data[0] == 0x80;
        if is_v0 {
            offset += 1; // skip version prefix byte
        }

        // --- Header (3 bytes) ---
        if offset + 3 > data.len() {
            return Err(TransactionError::TooShort);
        }
        let header = MessageHeader {
            num_required_signatures: data[offset],
            num_readonly_signed_accounts: data[offset + 1],
            num_readonly_unsigned_accounts: data[offset + 2],
        };
        offset += 3;

        // --- Account keys (compact-u16 count + N * 32 bytes) ---
        let (num_keys, consumed) = decode_compact_u16(data, offset)?;
        offset += consumed;

        if offset + num_keys * 32 > data.len() {
            return Err(TransactionError::TooShort);
        }
        let mut account_keys = Vec::with_capacity(num_keys);
        for _ in 0..num_keys {
            let mut key = [0u8; 32];
            key.copy_from_slice(&data[offset..offset + 32]);
            account_keys.push(Pubkey(key));
            offset += 32;
        }

        // --- Recent blockhash (32 bytes) ---
        if offset + 32 > data.len() {
            return Err(TransactionError::TooShort);
        }
        let mut recent_blockhash = [0u8; 32];
        recent_blockhash.copy_from_slice(&data[offset..offset + 32]);
        offset += 32;

        // --- Instructions (compact-u16 count + N instructions) ---
        let (num_instructions, consumed) = decode_compact_u16(data, offset)?;
        offset += consumed;

        let mut instructions = Vec::with_capacity(num_instructions);
        for _ in 0..num_instructions {
            if offset >= data.len() {
                return Err(TransactionError::TooShort);
            }

            // program_id_index (1 byte)
            let program_id_index = data[offset];
            offset += 1;

            // account indices (compact-u16 length + bytes)
            let (num_accounts, consumed) = decode_compact_u16(data, offset)?;
            offset += consumed;
            if offset + num_accounts > data.len() {
                return Err(TransactionError::TooShort);
            }
            let accounts = data[offset..offset + num_accounts].to_vec();
            offset += num_accounts;

            // instruction data (compact-u16 length + bytes)
            let (data_len, consumed) = decode_compact_u16(data, offset)?;
            offset += consumed;
            if offset + data_len > data.len() {
                return Err(TransactionError::TooShort);
            }
            let ix_data = data[offset..offset + data_len].to_vec();
            offset += data_len;

            instructions.push(CompiledInstruction {
                program_id_index,
                accounts,
                data: ix_data,
            });
        }

        Ok(ParsedMessage {
            header,
            account_keys,
            recent_blockhash,
            instructions,
            is_v0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pubkey_from_str_valid() {
        // The system program is all zeros -> base58 = "11111111111111111111111111111111"
        let pk = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        assert_eq!(pk, Pubkey::SYSTEM_PROGRAM);
    }

    #[test]
    fn test_pubkey_display_roundtrip() {
        let pk = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        let s = pk.to_string();
        let pk2 = Pubkey::from_str(&s).unwrap();
        assert_eq!(pk, pk2);
    }

    #[test]
    fn test_pubkey_invalid_base58() {
        assert!(Pubkey::from_str("not-valid-base58!!!").is_err());
    }

    #[test]
    fn test_pubkey_wrong_length() {
        // "1" decodes to a single zero byte
        assert!(Pubkey::from_str("1").is_err());
    }

    #[test]
    fn test_pubkey_serde_roundtrip() {
        let pk = Pubkey::from_str("11111111111111111111111111111111").unwrap();
        let json = serde_json::to_string(&pk).unwrap();
        let pk2: Pubkey = serde_json::from_str(&json).unwrap();
        assert_eq!(pk, pk2);
    }

    #[test]
    fn test_token_program_id_constant() {
        let expected = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
        assert_eq!(Pubkey::TOKEN_PROGRAM_ID, expected);
    }

    #[test]
    fn test_token_2022_program_id_constant() {
        let expected = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap();
        assert_eq!(Pubkey::TOKEN_2022_PROGRAM_ID, expected);
    }

    // -----------------------------------------------------------------------
    // compact-u16 tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_decode_compact_u16_single_byte() {
        // Value 1 encoded as single byte: 0x01
        let (val, consumed) = decode_compact_u16(&[0x01], 0).unwrap();
        assert_eq!(val, 1);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_decode_compact_u16_zero() {
        let (val, consumed) = decode_compact_u16(&[0x00], 0).unwrap();
        assert_eq!(val, 0);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_decode_compact_u16_127() {
        // 127 = 0x7F, fits in one byte
        let (val, consumed) = decode_compact_u16(&[0x7F], 0).unwrap();
        assert_eq!(val, 127);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_decode_compact_u16_128() {
        // 128 = 0x80 in first byte (0 data bits + continue), 0x01 in second
        let (val, consumed) = decode_compact_u16(&[0x80, 0x01], 0).unwrap();
        assert_eq!(val, 128);
        assert_eq!(consumed, 2);
    }

    #[test]
    fn test_decode_compact_u16_with_offset() {
        let data = [0xFF, 0x01]; // offset 1 -> value 1
        let (val, consumed) = decode_compact_u16(&data, 1).unwrap();
        assert_eq!(val, 1);
        assert_eq!(consumed, 1);
    }

    #[test]
    fn test_decode_compact_u16_empty() {
        assert!(decode_compact_u16(&[], 0).is_err());
    }

    // -----------------------------------------------------------------------
    // VersionedTransaction tests (compact-u16 format)
    // -----------------------------------------------------------------------

    #[test]
    fn test_versioned_transaction_from_bytes() {
        // Build a minimal fake transaction:
        // compact-u16(1) = 0x01, then 1 signature (all 0xFF), then message bytes
        let mut data = Vec::new();
        data.push(0x01); // compact-u16: sig count = 1
        data.extend_from_slice(&[0xFF; 64]); // signature
        data.extend_from_slice(&[0x80, 0x01, 0x02, 0x03]); // message bytes

        let tx = VersionedTransaction::from_bytes(&data).unwrap();
        assert_eq!(tx.signatures.len(), 1);
        assert_eq!(tx.signatures[0].0, [0xFF; 64]);
        assert_eq!(tx.message_bytes, vec![0x80, 0x01, 0x02, 0x03]);
    }

    #[test]
    fn test_versioned_transaction_empty_signatures() {
        let mut data = Vec::new();
        data.push(0x00); // compact-u16: sig count = 0
        data.extend_from_slice(&[0x01, 0x02]); // message bytes

        let tx = VersionedTransaction::from_bytes(&data).unwrap();
        assert!(tx.signatures.is_empty());
    }

    #[test]
    fn test_versioned_transaction_too_short() {
        assert!(VersionedTransaction::from_bytes(&[]).is_err());
    }

    #[test]
    fn test_versioned_transaction_truncated_signatures() {
        // Claims 1 signature but only has 10 bytes of signature data
        let mut data = Vec::new();
        data.push(0x01); // compact-u16: sig count = 1
        data.extend_from_slice(&[0xFF; 10]); // truncated signature
        assert!(VersionedTransaction::from_bytes(&data).is_err());
    }

    #[test]
    fn test_usdc_mint_pubkey() {
        // USDC mint address should parse correctly
        let pk = Pubkey::from_str("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
        assert_eq!(
            pk.to_string(),
            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v"
        );
    }

    // -----------------------------------------------------------------------
    // ParsedMessage tests
    // -----------------------------------------------------------------------

    /// Helper: build a minimal legacy message with the given instructions.
    fn build_legacy_message(
        account_keys: &[Pubkey],
        instructions: &[(u8, &[u8], &[u8])], // (program_id_index, accounts, data)
    ) -> Vec<u8> {
        let mut msg = vec![
            1,                        // num_required_signatures
            0,                        // num_readonly_signed
            1,                        // num_readonly_unsigned
            account_keys.len() as u8, // compact-u16 for small values
        ];

        // Account keys
        for key in account_keys {
            msg.extend_from_slice(&key.0);
        }

        // Recent blockhash (32 zero bytes)
        msg.extend_from_slice(&[0u8; 32]);

        // Instructions (compact-u16 count)
        msg.push(instructions.len() as u8);
        for (pid_index, accounts, data) in instructions {
            msg.push(*pid_index);
            msg.push(accounts.len() as u8); // compact-u16
            msg.extend_from_slice(accounts);
            msg.push(data.len() as u8); // compact-u16
            msg.extend_from_slice(data);
        }

        msg
    }

    #[test]
    fn test_parse_legacy_message_basic() {
        let keys = vec![Pubkey::SYSTEM_PROGRAM, Pubkey::TOKEN_PROGRAM_ID];
        let msg_bytes = build_legacy_message(&keys, &[(1, &[0], &[3, 0x01])]);

        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();
        assert!(!parsed.is_v0);
        assert_eq!(parsed.header.num_required_signatures, 1);
        assert_eq!(parsed.account_keys.len(), 2);
        assert_eq!(parsed.account_keys[0], Pubkey::SYSTEM_PROGRAM);
        assert_eq!(parsed.account_keys[1], Pubkey::TOKEN_PROGRAM_ID);
        assert_eq!(parsed.instructions.len(), 1);
        assert_eq!(parsed.instructions[0].program_id_index, 1);
        assert_eq!(parsed.instructions[0].accounts, vec![0]);
        assert_eq!(parsed.instructions[0].data, vec![3, 0x01]);
    }

    #[test]
    fn test_parse_v0_message() {
        let keys = vec![Pubkey::SYSTEM_PROGRAM];
        let legacy_bytes = build_legacy_message(&keys, &[(0, &[], &[])]);

        // Prepend version prefix 0x80 for V0
        let mut v0_bytes = vec![0x80];
        v0_bytes.extend_from_slice(&legacy_bytes);

        let parsed = ParsedMessage::from_bytes(&v0_bytes).unwrap();
        assert!(parsed.is_v0);
        assert_eq!(parsed.account_keys.len(), 1);
    }

    #[test]
    fn test_parse_message_multiple_instructions() {
        let keys = vec![
            Pubkey::SYSTEM_PROGRAM,
            Pubkey::TOKEN_PROGRAM_ID,
            Pubkey::TOKEN_2022_PROGRAM_ID,
        ];
        let msg_bytes =
            build_legacy_message(&keys, &[(0, &[1, 2], &[0x01, 0x02]), (1, &[0, 2], &[0x03])]);

        let parsed = ParsedMessage::from_bytes(&msg_bytes).unwrap();
        assert_eq!(parsed.instructions.len(), 2);
        assert_eq!(parsed.instructions[0].program_id_index, 0);
        assert_eq!(parsed.instructions[0].accounts, vec![1, 2]);
        assert_eq!(parsed.instructions[1].program_id_index, 1);
        assert_eq!(parsed.instructions[1].data, vec![0x03]);
    }

    #[test]
    fn test_parse_message_empty() {
        assert!(ParsedMessage::from_bytes(&[]).is_err());
    }

    #[test]
    fn test_transaction_parse_message_integration() {
        // Build a full transaction: compact-u16(1) + signature + legacy message
        let keys = vec![Pubkey::SYSTEM_PROGRAM, Pubkey::TOKEN_PROGRAM_ID];
        let msg_bytes = build_legacy_message(&keys, &[(1, &[0], &[3])]);

        let mut tx_data = Vec::new();
        tx_data.push(0x01); // compact-u16: 1 signature
        tx_data.extend_from_slice(&[0xAA; 64]); // signature
        tx_data.extend_from_slice(&msg_bytes);

        let tx = VersionedTransaction::from_bytes(&tx_data).unwrap();
        let parsed = tx.parse_message().unwrap();
        assert_eq!(parsed.account_keys.len(), 2);
        assert_eq!(parsed.instructions.len(), 1);
        assert_eq!(parsed.instructions[0].data, vec![3]);
    }
}
