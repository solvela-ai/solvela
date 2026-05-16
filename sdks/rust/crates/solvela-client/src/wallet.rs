use std::fmt;

use bip39::Mnemonic;
use solana_sdk::pubkey::Pubkey;
use solana_sdk::signature::Signature;
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use zeroize::Zeroizing;

use crate::error::WalletError;

/// A Solana wallet wrapping a `Keypair`.
///
/// Provides BIP39 mnemonic creation/import, base58 keypair import,
/// env-var import, signing, and secure key zeroization on drop.
///
/// The raw 64-byte keypair (secret || public) is stored in a
/// `Zeroizing<[u8; 64]>` so that the secret key material is wiped from
/// memory when the wallet is dropped (MEDIUM-1 from the security audit).
/// `solana_sdk::Keypair` does not expose its internal buffer mutably, so
/// we keep the source of truth in our owned buffer and reconstruct the
/// `Keypair` on demand.
pub struct Wallet {
    keypair_bytes: Zeroizing<[u8; 64]>,
}

impl Wallet {
    /// Construct a `Wallet` from already-validated 64-byte keypair material.
    fn from_validated_bytes(bytes: [u8; 64]) -> Self {
        Self {
            keypair_bytes: Zeroizing::new(bytes),
        }
    }

    /// Reconstruct the `Keypair` for signing operations.
    ///
    /// The keypair is built lazily from the zeroizing-on-drop byte buffer
    /// each time it's needed. The transient `Keypair` will be dropped at
    /// the end of the calling expression — `solana_sdk::Keypair` does not
    /// guarantee zeroization, so callers should keep its lifetime short.
    fn keypair_inner(&self) -> Keypair {
        // SAFETY: `keypair_bytes` is only ever populated through code paths
        // that have already validated the buffer is a well-formed Ed25519
        // keypair, so `try_from` cannot fail here.
        Keypair::try_from(&self.keypair_bytes[..])
            .expect("Wallet keypair bytes are validated at construction time")
    }

    /// Create a new wallet with a fresh keypair, returning the wallet
    /// and its 12-word BIP39 mnemonic for backup.
    ///
    /// # Panics
    ///
    /// Panics if BIP39 mnemonic generation or keypair derivation fails,
    /// which should not occur under normal conditions.
    #[must_use]
    pub fn create() -> (Self, String) {
        // SAFETY: BIP39 12-word generation draws from system entropy and is
        // infallible in practice; no precondition can cause it to fail.
        let mnemonic = Mnemonic::generate(12).expect("mnemonic generation should not fail");
        let phrase = mnemonic.to_string();
        let seed = mnemonic.to_seed("");
        let kp_bytes = keypair_bytes_from_seed(&seed);
        // SAFETY: `keypair_bytes_from_seed` always returns a valid 64-byte
        // Ed25519 keypair derived from a well-formed BIP39 seed.
        Keypair::try_from(kp_bytes.as_slice()).expect("keypair from valid seed should not fail");
        (Self::from_validated_bytes(kp_bytes), phrase)
    }

    /// Restore a wallet from a BIP39 mnemonic phrase.
    ///
    /// # Errors
    ///
    /// Returns `WalletError::InvalidMnemonic` if the phrase is not valid BIP39.
    pub fn from_mnemonic(phrase: &str) -> Result<Self, WalletError> {
        let mnemonic: Mnemonic = phrase
            .parse()
            .map_err(|e: bip39::Error| WalletError::InvalidMnemonic(e.to_string()))?;
        let seed = mnemonic.to_seed("");
        let kp_bytes = keypair_bytes_from_seed(&seed);
        Keypair::try_from(kp_bytes.as_slice())
            .map_err(|e| WalletError::InvalidMnemonic(e.to_string()))?;
        Ok(Self::from_validated_bytes(kp_bytes))
    }

    /// Import a wallet from a base58-encoded 64-byte keypair.
    ///
    /// # Errors
    ///
    /// Returns `WalletError::InvalidKeypair` if the string is not valid
    /// base58 or does not decode to a valid 64-byte keypair.
    pub fn from_keypair_b58(b58: &str) -> Result<Self, WalletError> {
        let bytes = bs58::decode(b58)
            .into_vec()
            .map_err(|e| WalletError::InvalidKeypair(e.to_string()))?;
        Self::from_keypair_bytes(&bytes)
    }

    /// Import a wallet from raw keypair bytes (64 bytes: 32 secret + 32 public).
    ///
    /// This accepts the format produced by `solana-keygen` JSON files
    /// after parsing the JSON array into bytes.
    ///
    /// # Errors
    ///
    /// Returns `WalletError::InvalidKeypair` if the bytes are not a valid Ed25519 keypair.
    pub fn from_keypair_bytes(bytes: &[u8]) -> Result<Self, WalletError> {
        // Validate as a real keypair first so we never store junk.
        Keypair::try_from(bytes).map_err(|e| WalletError::InvalidKeypair(e.to_string()))?;

        let fixed: [u8; 64] = bytes
            .try_into()
            .map_err(|_| WalletError::InvalidKeypair("expected exactly 64 bytes".to_string()))?;
        Ok(Self::from_validated_bytes(fixed))
    }

    /// Import a wallet from an environment variable containing a base58 keypair.
    ///
    /// # Errors
    ///
    /// Returns `WalletError::EnvNotSet` if the variable is not set, or
    /// `WalletError::InvalidKeypair` if the value is not a valid keypair.
    pub fn from_env(var: &str) -> Result<Self, WalletError> {
        let val = std::env::var(var).map_err(|_| WalletError::EnvNotSet(var.to_string()))?;
        Self::from_keypair_b58(&val)
    }

    /// Returns the base58 public key address of this wallet.
    #[must_use]
    pub fn address(&self) -> String {
        self.pubkey().to_string()
    }

    /// Returns the Solana `Pubkey` of this wallet.
    ///
    /// The public key occupies bytes 32..64 of the stored keypair.
    #[must_use]
    pub fn pubkey(&self) -> Pubkey {
        let mut public = [0u8; 32];
        public.copy_from_slice(&self.keypair_bytes[32..]);
        Pubkey::from(public)
    }

    /// Sign a message, returning a Solana `Signature`.
    #[allow(dead_code)] // used in tests only; reserved for future message-signing API
    pub(crate) fn sign(&self, message: &[u8]) -> Signature {
        self.keypair_inner().sign_message(message)
    }

    /// Return the raw 64-byte keypair (secret || public) for serialization.
    ///
    /// The caller is responsible for zeroizing the returned bytes when done.
    #[must_use]
    pub fn to_keypair_bytes(&self) -> [u8; 64] {
        *self.keypair_bytes
    }

    /// Return the keypair as a base58-encoded string.
    ///
    /// The caller is responsible for zeroizing the returned string when done.
    #[must_use]
    pub fn to_keypair_b58(&self) -> String {
        bs58::encode(*self.keypair_bytes).into_string()
    }

    /// Access the inner keypair for transaction signing.
    ///
    /// Returns an owned `Keypair` reconstructed from the zeroizing buffer.
    /// `solana_sdk::Keypair` itself does not zero on drop, so callers should
    /// keep the returned value's scope as small as possible.
    pub(crate) fn keypair(&self) -> Keypair {
        self.keypair_inner()
    }
}

impl fmt::Debug for Wallet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Wallet({})", self.address())
    }
}

// `Drop` is intentionally not implemented: `keypair_bytes` is wrapped in
// `zeroize::Zeroizing`, which performs in-place zeroization of the actual
// stored buffer when the wallet is dropped (MEDIUM-1).

/// Derive a 64-byte keypair (secret || public) from the first 32 bytes of a BIP39 seed.
fn keypair_bytes_from_seed(seed: &[u8]) -> [u8; 64] {
    let secret = &seed[..32];
    // SAFETY: BIP39 seeds are always ≥64 bytes; the first 32 bytes are a
    // valid Ed25519 secret key. The `try_into` to `[u8; 32]` cannot fail.
    let signing_key = ed25519_dalek::SigningKey::from_bytes(
        secret.try_into().expect("seed should be at least 32 bytes"),
    );
    let public = ed25519_dalek::VerifyingKey::from(&signing_key);
    let mut result = [0u8; 64];
    result[..32].copy_from_slice(secret);
    result[32..].copy_from_slice(public.as_bytes());
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_create_returns_valid_wallet_and_mnemonic() {
        let (wallet, mnemonic) = Wallet::create();
        let word_count = mnemonic.split_whitespace().count();
        assert!(
            word_count == 12 || word_count == 24,
            "got {word_count} words"
        );
        let addr = wallet.address();
        assert!(!addr.is_empty());
        assert!(bs58::decode(&addr).into_vec().is_ok());
    }

    #[test]
    fn test_wallet_from_mnemonic_roundtrip() {
        let (original, mnemonic) = Wallet::create();
        let restored = Wallet::from_mnemonic(&mnemonic).unwrap();
        assert_eq!(original.address(), restored.address());
    }

    #[test]
    fn test_wallet_from_mnemonic_invalid() {
        let result = Wallet::from_mnemonic("invalid mnemonic phrase");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WalletError::InvalidMnemonic(_)
        ));
    }

    #[test]
    fn test_wallet_from_keypair_bytes() {
        let kp = Keypair::new();
        let b58 = bs58::encode(kp.to_bytes()).into_string();
        let wallet = Wallet::from_keypair_b58(&b58).unwrap();
        assert_eq!(wallet.address(), kp.pubkey().to_string());
    }

    #[test]
    fn test_wallet_from_keypair_b58_invalid() {
        let result = Wallet::from_keypair_b58("not-valid-base58!!!");
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WalletError::InvalidKeypair(_)
        ));
    }

    #[test]
    fn test_wallet_from_env_not_set() {
        let result = Wallet::from_env("SOLVELA_TEST_NONEXISTENT_VAR_12345");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), WalletError::EnvNotSet(_)));
    }

    #[test]
    fn test_wallet_debug_redacts_key() {
        let (wallet, _) = Wallet::create();
        let debug = format!("{wallet:?}");
        assert!(debug.contains("Wallet("));
        assert!(debug.contains(&wallet.address()));
        assert!(!debug.contains("keypair"));
        assert!(!debug.contains("secret"));
    }

    #[test]
    fn test_wallet_sign_produces_valid_signature() {
        let (wallet, _) = Wallet::create();
        let message = b"test message";
        let sig = wallet.sign(message);
        assert_eq!(sig.as_ref().len(), 64);
    }

    #[test]
    fn test_wallet_pubkey_returns_solana_pubkey() {
        let (wallet, _) = Wallet::create();
        let pubkey = wallet.pubkey();
        assert_eq!(pubkey.to_string(), wallet.address());
    }

    #[test]
    fn test_from_keypair_bytes_roundtrip() {
        let (wallet, _) = Wallet::create();
        let bytes = wallet.keypair().to_bytes();
        let restored = Wallet::from_keypair_bytes(&bytes).unwrap();
        assert_eq!(wallet.address(), restored.address());
    }

    #[test]
    fn test_from_keypair_bytes_invalid() {
        let result = Wallet::from_keypair_bytes(&[0u8; 32]); // too short
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            WalletError::InvalidKeypair(_)
        ));
    }

    #[test]
    fn test_two_wallets_different_addresses() {
        let (w1, _) = Wallet::create();
        let (w2, _) = Wallet::create();
        assert_ne!(w1.address(), w2.address());
    }
}
