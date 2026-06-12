use async_trait::async_trait;

use crate::types::{PaymentPayload, SettlementResult, VerificationResult};

/// Chain-agnostic payment verification and settlement trait.
///
/// This trait abstracts the chain-specific logic for verifying and settling
/// x402 payments. The Solana implementation is the primary (and currently only)
/// implementation. Designed for future multi-chain support (e.g., EVM/Base).
#[async_trait]
pub trait PaymentVerifier: Send + Sync {
    /// Returns the network identifier (e.g., "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp").
    fn network(&self) -> &str;

    /// Returns the payment scheme this verifier handles (e.g. "exact", "escrow").
    fn scheme(&self) -> &str;

    /// Verify that a payment payload is valid without settling it.
    ///
    /// Checks:
    /// - Transaction is well-formed and properly signed
    /// - SPL Token transfer instruction targets the correct recipient
    /// - Transfer amount meets the required minimum
    /// - Transaction simulation succeeds
    async fn verify_payment(&self, payload: &PaymentPayload) -> Result<VerificationResult, Error>;

    /// Verify a payment against a CALLER-SUPPLIED expected recipient instead
    /// of the verifier's statically-configured one.
    ///
    /// Used by the service marketplace's per-service `vendor_wallet`
    /// (settlement-platform P1): a service with a vendor wallet must be paid
    /// to that wallet, not the gateway's global recipient, with the same hard
    /// recipient-equality check `verify_payment` applies.
    ///
    /// The default implementation FAILS CLOSED with
    /// [`Error::RecipientOverrideUnsupported`]: a verifier that has not
    /// implemented per-request recipient override must never silently verify
    /// against its static recipient when the caller asked for a different one
    /// — that would misdirect funds.
    async fn verify_payment_to(
        &self,
        payload: &PaymentPayload,
        expected_recipient: &str,
    ) -> Result<VerificationResult, Error> {
        let _ = (payload, expected_recipient);
        Err(Error::RecipientOverrideUnsupported(format!(
            "{}/{}",
            self.network(),
            self.scheme()
        )))
    }

    /// Settle a verified payment by broadcasting the transaction on-chain.
    ///
    /// Steps:
    /// 1. Broadcast via `sendTransaction` RPC
    /// 2. Confirm via `confirmTransaction` RPC
    /// 3. Verify post-tx token balances
    async fn settle_payment(&self, payload: &PaymentPayload) -> Result<SettlementResult, Error>;
}

/// Errors that can occur during payment verification or settlement.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid transaction encoding: {0}")]
    InvalidEncoding(String),

    #[error("invalid transaction: {0}")]
    InvalidTransaction(String),

    #[error("invalid signature: {0}")]
    InvalidSignature(String),

    #[error("insufficient payment: expected {expected} atomic units, got {actual}")]
    InsufficientPayment { expected: u64, actual: u64 },

    #[error("wrong recipient: expected {expected}, got {actual}")]
    WrongRecipient { expected: String, actual: String },

    #[error("wrong asset: expected {expected}, got {actual}")]
    WrongAsset { expected: String, actual: String },

    #[error("transaction simulation failed: {0}")]
    SimulationFailed(String),

    #[error("settlement failed: {0}")]
    SettlementFailed(String),

    #[error("rpc error: {0}")]
    Rpc(String),

    #[error("timeout waiting for confirmation")]
    Timeout,

    #[error("unsupported network: {0}")]
    UnsupportedNetwork(String),

    #[error("escrow deposit not confirmed: {0}")]
    EscrowNotConfirmed(String),

    #[error("escrow claim failed: {0}")]
    EscrowClaimFailed(String),

    #[error("payload type mismatch: {0}")]
    PayloadMismatch(String),

    #[error(
        "verifier {0} does not support per-request recipient override; \
         refusing to verify against its static recipient"
    )]
    RecipientOverrideUnsupported(String),
}
