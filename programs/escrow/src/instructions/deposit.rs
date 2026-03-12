use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Mint, Token, TokenAccount, Transfer};

use crate::errors::EscrowError;
use crate::state::Escrow;
use crate::USDC_MINT;

/// Deposit USDC into the PDA vault, locking funds for a specific service request.
///
/// The agent pre-authorises a maximum payment amount. After the service is
/// delivered, the provider calls `claim` for the actual cost; any remainder
/// is automatically refunded. If the service is not delivered before
/// `expiry_slot`, the agent can call `refund` to reclaim funds.
pub fn deposit(
    ctx: Context<Deposit>,
    amount: u64,
    service_id: [u8; 32],
    expiry_slot: u64,
) -> Result<()> {
    require!(amount > 0, EscrowError::ZeroAmount);
    require!(expiry_slot > Clock::get()?.slot, EscrowError::InvalidExpiry);

    let escrow = &mut ctx.accounts.escrow;
    escrow.agent = ctx.accounts.agent.key();
    escrow.provider = ctx.accounts.provider.key();
    escrow.mint = ctx.accounts.mint.key();
    escrow.amount = amount;
    escrow.service_id = service_id;
    escrow.expiry_slot = expiry_slot;
    escrow.bump = ctx.bumps.escrow;

    // Transfer USDC from agent's ATA → vault ATA
    let cpi_accounts = Transfer {
        from: ctx.accounts.agent_token_account.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.agent.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::transfer(cpi_ctx, amount)?;

    emit!(DepositEvent {
        agent: escrow.agent,
        provider: escrow.provider,
        amount,
        service_id,
        expiry_slot,
    });

    Ok(())
}

#[derive(Accounts)]
#[instruction(amount: u64, service_id: [u8; 32])]
pub struct Deposit<'info> {
    /// Agent wallet — pays for the transaction and provides the funds.
    #[account(mut)]
    pub agent: Signer<'info>,

    /// Service provider wallet — receives payment after service delivery.
    /// CHECK: validated by the provider's identity at claim time; no on-chain
    /// constraint needed here.
    pub provider: UncheckedAccount<'info>,

    /// USDC mint — only the mainnet USDC mint is accepted.
    #[account(address = USDC_MINT)]
    pub mint: Account<'info, Mint>,

    /// Escrow PDA — stores deposit metadata.
    #[account(
        init,
        payer = agent,
        space = 8 + Escrow::INIT_SPACE,
        seeds = [b"escrow", agent.key().as_ref(), &service_id],
        bump,
    )]
    pub escrow: Account<'info, Escrow>,

    /// Agent's associated token account (source of funds).
    #[account(
        mut,
        associated_token::mint = mint,
        associated_token::authority = agent,
    )]
    pub agent_token_account: Account<'info, TokenAccount>,

    /// Vault ATA owned by the escrow PDA (destination of funds).
    #[account(
        init_if_needed,
        payer = agent,
        associated_token::mint = mint,
        associated_token::authority = escrow,
    )]
    pub vault: Account<'info, TokenAccount>,

    pub token_program: Program<'info, Token>,
    pub associated_token_program: Program<'info, AssociatedToken>,
    pub system_program: Program<'info, System>,
}

#[event]
pub struct DepositEvent {
    pub agent: Pubkey,
    pub provider: Pubkey,
    pub amount: u64,
    pub service_id: [u8; 32],
    pub expiry_slot: u64,
}
