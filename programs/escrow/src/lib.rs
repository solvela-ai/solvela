//! Solvela Escrow Program
//!
//! Trustless USDC-SPL escrow for AI agent payment settlement on Solana.
//!
//! ## Payment Flow
//!
//! ```text
//! Agent → deposit(max_amount, service_id, expiry_slot)
//!              → USDC locked in PDA vault
//!
//! Gateway delivers LLM response
//!
//! Gateway → claim(actual_cost)
//!              → actual_cost → provider ATA
//!              → (max_amount - actual_cost) → agent ATA  (refund)
//!              → vault + escrow accounts closed
//!
//! OR (if gateway fails / times out):
//!
//! Agent → refund()  [only after expiry_slot]
//!              → full deposit → agent ATA
//!              → vault + escrow accounts closed
//! ```
//!
//! ## PDA Seeds
//!
//! `[b"escrow", agent.key().as_ref(), service_id]`
//!
//! `service_id` is a 32-byte request correlation ID (e.g. SHA-256 of request
//! body) so each API call gets its own independent escrow account.

use anchor_lang::prelude::*;

pub mod errors;
pub mod instructions;
pub mod state;

use instructions::*;

/// USDC-SPL mint address the escrow accepts. Feature-gated so the same
/// program can be built for mainnet, devnet, or local test validators
/// without source edits. The deployed mainnet bytecode at the program ID
/// below MUST be built with `--features mainnet`; default builds (e.g.
/// `cargo check --lib`, `anchor test` against a local validator) target
/// the devnet USDC mint.
#[cfg(feature = "mainnet")]
pub const USDC_MINT: Pubkey = pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");
#[cfg(not(feature = "mainnet"))]
pub const USDC_MINT: Pubkey = pubkey!("4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU");

/// Maximum allowed gap between current slot and `expiry_slot` at deposit
/// time. ~1 day at 400ms/slot — generous for any realistic API request
/// (typical x402 calls resolve in seconds; this only constrains pathological
/// clients that would otherwise lock funds for years).
pub const MAX_ESCROW_SLOTS: u64 = 216_000;

declare_id!("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");

#[program]
pub mod solvela_escrow {
    use super::*;

    /// Agent deposits USDC into PDA vault.
    ///
    /// # Arguments
    ///
    /// * `amount`      - USDC amount in micro-units (6 decimals)
    /// * `service_id`  - 32-byte request correlation ID
    /// * `expiry_slot` - Slot after which the agent may call `refund`
    pub fn deposit(
        ctx: Context<Deposit>,
        amount: u64,
        service_id: [u8; 32],
        expiry_slot: u64,
    ) -> Result<()> {
        instructions::deposit::deposit(ctx, amount, service_id, expiry_slot)
    }

    /// Service provider claims payment after delivering the service.
    ///
    /// # Arguments
    ///
    /// * `actual_amount` - Actual cost charged (≤ deposited amount)
    pub fn claim(ctx: Context<Claim>, actual_amount: u64) -> Result<()> {
        instructions::claim::claim(ctx, actual_amount)
    }

    /// Agent reclaims funds if service not delivered before `expiry_slot`.
    pub fn refund(ctx: Context<Refund>) -> Result<()> {
        instructions::refund::refund(ctx)
    }
}
