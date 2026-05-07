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
    // Critical: gate the claim window on `slot < expiry_slot`. Without this,
    // claim and refund are simultaneously valid once `slot >= expiry_slot`,
    // and an adversarial provider can race the agent at the boundary. The
    // entire deterministic-deadline guarantee for the agent depends on this
    // line. See refund.rs for the matching `slot >= expiry_slot` guard.
    require!(
        Clock::get()?.slot < ctx.accounts.escrow.expiry_slot,
        EscrowError::EscrowExpired,
    );

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

    // Refund remainder → agent ATA. The `actual_amount <= escrow.amount`
    // guard above already prevents underflow, but `checked_sub` makes the
    // invariant explicit at the call site (and survives a future refactor
    // that moves or removes the guard).
    let refund = escrow
        .amount
        .checked_sub(actual_amount)
        .ok_or(EscrowError::ClaimExceedsDeposit)?;
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
        deposited: escrow.amount,
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
    /// CHECK: `address = escrow.agent` is the only constraint that matters.
    /// Using `UncheckedAccount` (vs `SystemAccount`) keeps the door open for
    /// PDA- or program-owned agents (e.g. a future "agent vault" routing
    /// through CPI). The address constraint above is equivalent in safety
    /// — it pins the key to whatever was stored at deposit time.
    #[account(mut, address = escrow.agent)]
    pub agent: UncheckedAccount<'info>,

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

    /// Agent's ATA — receives the refund remainder. `init_if_needed` so a
    /// claim still succeeds when the agent has closed their ATA between
    /// deposit and claim (a normal post-deposit cleanup move that reclaims
    /// rent). Provider pays for recreation since they're already the
    /// transaction signer with funds.
    #[account(
        init_if_needed,
        payer = provider,
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
    /// Original deposit amount. Surfaced so downstream indexers can tell a
    /// partial claim from a full one without recomputing `claimed + refunded`.
    pub deposited: u64,
    pub claimed: u64,
    pub refunded: u64,
    pub service_id: [u8; 32],
}
