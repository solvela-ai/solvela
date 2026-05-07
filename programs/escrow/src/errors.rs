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

    // New variants appended below preserve wire codes for the variants above.
    // Anchor encodes the error code as the discriminant (declaration order),
    // so anything inserted earlier would shift the indexed values for clients.
    /// Escrow has expired; claim is no longer permitted.
    #[msg("Escrow has expired; claim no longer permitted")]
    EscrowExpired,

    /// Expiry slot is further in the future than the program permits.
    #[msg("Expiry slot exceeds the maximum allowed window")]
    ExpiryTooFar,

    /// Provider key is invalid (zero key, or equal to the agent).
    #[msg("Provider must be a non-default key distinct from the agent")]
    InvalidProvider,

    /// Vault held zero tokens at refund time.
    #[msg("Vault is empty; nothing to refund")]
    EmptyVault,
}
