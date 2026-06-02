use serde::{Deserialize, Serialize};

/// Result of payment verification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationResult {
    /// Whether the payment is valid.
    pub valid: bool,
    /// Human-readable reason if invalid.
    pub reason: Option<String>,
    /// Verified amount in atomic units.
    pub verified_amount: Option<u64>,
}

/// Why a settlement failed, used to decide whether a retry could ever succeed.
///
/// The gateway maps this to the client-facing message: a `Rejected` payment is
/// a dead end (telling the agent to "please retry" wastes its time and tokens),
/// whereas `Timeout`/`Submission` are genuinely worth retrying.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementFailureKind {
    /// The transaction was deterministically rejected by the chain — either at
    /// preflight or after landing with an on-chain `InstructionError`. The same
    /// signed transaction can NEVER confirm on retry. `program_error_code`
    /// carries only the numeric program error (e.g. an Anchor constraint code
    /// such as `2012`/`ConstraintAddress`); it deliberately never carries raw
    /// RPC internals (RPC URL, full error JSON) — see GHSA-cgqx-mg48-949v.
    Rejected {
        /// Numeric on-chain program error code, when one could be extracted.
        /// Solana program error codes are `u32` (e.g. Anchor's `2012` /
        /// `ConstraintAddress`); `None` when the rejection carried no `Custom`
        /// code (a builtin instruction error) or none could be parsed.
        program_error_code: Option<u32>,
    },
    /// Submitted but not confirmed within the budget, with no on-chain error
    /// observed. The blockhash may simply have been slow to land — retry may work.
    Timeout,
    /// Submission failed at the RPC/transport layer (network blip, RPC down,
    /// expired blockhash). Retry may work.
    Submission,
}

/// Result of payment settlement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettlementResult {
    /// Whether settlement was successful.
    pub success: bool,
    /// Transaction signature (base58 for Solana).
    pub tx_signature: Option<String>,
    /// Network the settlement occurred on.
    pub network: String,
    /// Error message if settlement failed.
    pub error: Option<String>,
    /// Verified deposit amount in atomic units (escrow scheme only).
    /// Used to cap the claim amount so it never exceeds the deposited amount.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub verified_amount: Option<u64>,
    /// Classification of the failure when `success` is false; `None` on success
    /// or when the failure was not classified. Lets the gateway distinguish a
    /// permanent rejection from a transient timeout when shaping the client error.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(default)]
    pub failure_kind: Option<SettlementFailureKind>,
}
