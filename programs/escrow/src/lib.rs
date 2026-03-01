//! RustyClawRouter Escrow Program
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

// Placeholder program ID — replace with output of `anchor build` + `anchor keys list`
declare_id!("GTs7ik3NbW3xwSXq33jyVRGgmshNEyW1h9rxDNATiFLy");

#[program]
pub mod rustyclawrouter_escrow {
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
