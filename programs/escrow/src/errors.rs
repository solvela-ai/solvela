use anchor_lang::prelude::*;

/// Escrow program error codes.
#[error_code]
pub enum EscrowError {
    /// The claimed amount exceeds what was deposited.
    #[msg("Claimed amount exceeds deposited amount")]
    ClaimExceedsDeposit,

    /// The escrow has not expired yet; refund is not allowed.
    #[msg("Escrow has not expired; refund not yet available")]
    EscrowNotExpired,

    /// The provided mint does not match the deposited mint.
    #[msg("Mint mismatch: expected deposited mint")]
    MintMismatch,

    /// Amount must be greater than zero.
    #[msg("Amount must be greater than zero")]
    ZeroAmount,

    /// Escrow expiry slot must be in the future.
    #[msg("Expiry slot must be in the future")]
    InvalidExpiry,
}
