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
}
