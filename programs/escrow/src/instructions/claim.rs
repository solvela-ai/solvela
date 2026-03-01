use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, CloseAccount, Mint, Token, TokenAccount, Transfer};

use crate::errors::EscrowError;
use crate::state::Escrow;

/// Claim payment from the escrow vault after delivering the service.
///
/// The provider specifies `actual_amount` (≤ deposited amount). The gateway:
///   1. Transfers `actual_amount` from vault → provider ATA
///   2. Transfers remainder (deposit − actual) from vault → agent ATA (refund)
///   3. Closes the vault and escrow accounts (rent returned to agent)
pub fn claim(ctx: Context<Claim>, actual_amount: u64) -> Result<()> {
    require!(
        actual_amount <= ctx.accounts.escrow.amount,
        EscrowError::ClaimExceedsDeposit
    );
    require!(actual_amount > 0, EscrowError::ZeroAmount);

    let escrow = &ctx.accounts.escrow;
    let seeds: &[&[u8]] = &[
        b"escrow",
        escrow.agent.as_ref(),
        &escrow.service_id,
        &[escrow.bump],
    ];
    let signer_seeds = &[seeds];

    // Transfer actual_amount → provider ATA
    let cpi_transfer = CpiContext::new_with_signer(
        ctx.accounts.token_program.to_account_info(),
        Transfer {
            from: ctx.accounts.vault.to_account_info(),
            to: ctx.accounts.provider_token_account.to_account_info(),
            authority: ctx.accounts.escrow.to_account_info(),
        },
        signer_seeds,
    );
    token::transfer(cpi_transfer, actual_amount)?;

    // Refund remainder → agent ATA
    let refund = escrow.amount - actual_amount;
    if refund > 0 {
        let cpi_refund = CpiContext::new_with_signer(
            ctx.accounts.token_program.to_account_info(),
            Transfer {
                from: ctx.accounts.vault.to_account_info(),
                to: ctx.accounts.agent_token_account.to_account_info(),
                authority: ctx.accounts.escrow.to_account_info(),
            },
            signer_seeds,
        );
        token::transfer(cpi_refund, refund)?;
    }

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

    emit!(ClaimEvent {
        agent: escrow.agent,
        provider: escrow.provider,
        claimed: actual_amount,
        refunded: refund,
        service_id: escrow.service_id,
    });

    Ok(())
}

#[derive(Accounts)]
pub struct Claim<'info> {
    /// Escrow PDA — validated by seeds; closed to agent on success.
    #[account(
        mut,
        seeds = [b"escrow", escrow.agent.as_ref(), &escrow.service_id],
        bump = escrow.bump,
        close = agent,
        has_one = provider,
        has_one = mint,
    )]
    pub escrow: Account<'info, Escrow>,

    /// Agent wallet — receives vault rent refund and any token remainder.
    /// Validated via `escrow.agent` address stored on-chain.
    #[account(mut, address = escrow.agent)]
    pub agent: SystemAccount<'info>,

    /// Provider wallet — must match `escrow.provider`; pays for ATA init.
    #[account(mut)]
    pub provider: Signer<'info>,

    /// USDC mint — validated via `has_one = mint`.
    pub mint: Account<'info, Mint>,

    /// Vault ATA (escrow PDA authority).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub vault: Account<'info, TokenAccount>,

    /// Provider's ATA — receives the claimed payment.
    #[account(
        init_if_needed,
        payer = provider,
        associated_token::mint = mint,
        associated_token::authority = provider,
    )]
    pub provider_token_account: Account<'info, TokenAccount>,

    /// Agent's ATA — receives the refund remainder.
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
pub struct ClaimEvent {
    pub agent: Pubkey,
    pub provider: Pubkey,
    pub claimed: u64,
    pub refunded: u64,
    pub service_id: [u8; 32],
}
