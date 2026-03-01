use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, CloseAccount, Mint, Token, TokenAccount, Transfer};

use crate::errors::EscrowError;
use crate::state::Escrow;

/// Reclaim deposited funds if the service was not delivered before `expiry_slot`.
///
/// The agent may call this after `Clock::slot >= escrow.expiry_slot`.
/// All deposited tokens are returned to the agent's ATA, and both the vault
/// and escrow accounts are closed (rent returned to agent).
pub fn refund(ctx: Context<Refund>) -> Result<()> {
    let clock = Clock::get()?;
    require!(
        clock.slot >= ctx.accounts.escrow.expiry_slot,
        EscrowError::EscrowNotExpired
    );

    let escrow = &ctx.accounts.escrow;
    let seeds: &[&[u8]] = &[
        b"escrow",
        escrow.agent.as_ref(),
        &escrow.service_id,
        &[escrow.bump],
    ];
    let signer_seeds = &[seeds];

    // Return all vault tokens → agent ATA
    let vault_amount = ctx.accounts.vault.amount;
    let cpi_transfer = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.agent_token_account.to_account_info(),
            authority: ctx.accounts.escrow.to_account_info(),
        },
        signer_seeds,
    );
    token::transfer(cpi_transfer, vault_amount)?;

    // Close vault ATA (returns rent to agent)
    let cpi_close = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        CloseAccount {
            account: ctx.accounts.vault.to_account_info(),
            destination: ctx.accounts.agent.to_account_info(),
            authority: ctx.accounts.escrow.to_account_info(),
        },
        signer_seeds,
    );
    token::close_account(cpi_close)?;

    emit!(RefundEvent {
        agent: escrow.agent,
        refunded: vault_amount,
        service_id: escrow.service_id,
        expiry_slot: escrow.expiry_slot,
        current_slot: clock.slot,
    });

    Ok(())
}

#[derive(Accounts)]
pub struct Refund<'info> {
    /// Escrow PDA — validated by seeds.
    #[account(
        mut,
        seeds = [b"escrow", escrow.agent.as_ref(), &escrow.service_id],
        bump = escrow.bump,
        close = agent,
        has_one = mint,
    )]
    pub escrow: Account<'info, Escrow>,

    /// Agent wallet — must match `escrow.agent`; receives rent and refund.
    #[account(mut, address = escrow.agent)]
    pub agent: Signer<'info>,

    /// USDC mint — validated via `has_one = mint`.
    pub mint: Account<'info, Mint>,

    /// Vault ATA (escrow PDA authority).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub vault: Account<'info, TokenAccount>,

    /// Agent's ATA — destination for refunded tokens.
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = agent,
    )]
    pub agent_token_account: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[event]
pub struct RefundEvent {
    pub agent: Pubkey,
    pub refunded: u64,
    pub service_id: [u8; 32],
    pub expiry_slot: u64,
    pub current_slot: u64,
}
