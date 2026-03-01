use anchor_lang::prelude::*;

/// On-chain escrow account storing deposit metadata.
///
/// PDA seeds: `[b"escrow", agent.key().as_ref(), service_id]`
/// Size: 8 (discriminator) + InitSpace
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    /// Agent (depositor) wallet public key.
    pub agent: Pubkey,
    /// Service provider wallet public key (receives payment on claim).
    pub provider: Pubkey,
    /// SPL token mint (must be USDC: EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v).
    pub mint: Pubkey,
    /// Deposited amount in token base units (USDC: 6 decimals).
    pub amount: u64,
    /// 32-byte service/request correlation ID (e.g. SHA-256 of request body).
    pub service_id: [u8; 32],
    /// Slot at which the agent may reclaim funds if service is not delivered.
    /// Must be > current slot at deposit time.
    pub expiry_slot: u64,
    /// PDA bump seed.
    pub bump: u8,
}
