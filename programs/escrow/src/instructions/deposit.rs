use anchor_lang::prelude::*;
use anchor_spl::associated_token::AssociatedToken;
use anchor_spl::token::{self, Mint, Token, TokenAccount, TransferChecked};

use crate::errors::EscrowError;
use crate::state::Escrow;
use crate::{MAX_ESCROW_SLOTS, MIN_EXPIRY_BUFFER, USDC_MINT};

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

    let now = Clock::get()?.slot;
    // `expiry_slot` semantically denotes the *first expired slot*. At deposit
    // time we require strict `expiry_slot > now` so the escrow is born with
    // at least one active slot. Pairs symmetrically with the claim
    // (`slot < expiry_slot`) and refund (`slot >= expiry_slot`) guards: at
    // `slot == expiry_slot` claim fails and refund succeeds, so creating an
    // escrow with `expiry_slot == now` would mean it's born already expired.
    // The boundary case is locked in by
    // `tests/integration.rs::test_deposit_expiry_equal_now_fails`.
    require!(expiry_slot > now, EscrowError::InvalidExpiry);
    // Cap the deposit-to-expiry gap so a buggy or adversarial client can't
    // pass `expiry_slot = u64::MAX` and lock the agent out of refund forever.
    // saturating_sub guards against the (already-rejected) `expiry_slot <= now`
    // case in case of refactor; the `>` check above means subtraction is safe.
    require!(
        expiry_slot.saturating_sub(now) <= MAX_ESCROW_SLOTS,
        EscrowError::ExpiryTooFar,
    );
    // Floor the deposit-to-expiry gap so an adversarial agent can't set
    // `expiry_slot = now + 1`, receive the LLM response (which the gateway
    // delivers HTTP-side immediately after settlement), and refund before
    // the gateway's claim transaction can confirm. ~20 s buffer at
    // 400ms/slot. The off-chain verifier in
    // `crates/x402/src/escrow/verifier.rs` enforces an equivalent buffer.
    require!(
        expiry_slot.saturating_sub(now) >= MIN_EXPIRY_BUFFER,
        EscrowError::ExpiryTooSoon,
    );

    // Provider field is `UncheckedAccount` (no on-chain owner constraint), so
    // the only on-chain protection a misconfigured client gets is this guard:
    // a zero-key or self-key provider can never legitimately claim, which
    // (combined with the expiry guard on claim) would brick the funds. Reject
    // both at deposit time.
    let provider_key = ctx.accounts.provider.key();
    require!(
        provider_key != Pubkey::default(),
        EscrowError::InvalidProvider,
    );
    require!(
        provider_key != ctx.accounts.agent.key(),
        EscrowError::InvalidProvider,
    );

    let escrow = &mut ctx.accounts.escrow;
    escrow.agent = ctx.accounts.agent.key();
    escrow.provider = ctx.accounts.provider.key();
    escrow.mint = ctx.accounts.mint.key();
    escrow.amount = amount;
    escrow.service_id = service_id;
    escrow.expiry_slot = expiry_slot;
    escrow.bump = ctx.bumps.escrow;

    // Transfer USDC from agent's ATA → vault ATA. `TransferChecked`
    // (over plain `Transfer`) passes the mint and its decimals through
    // to the SPL token program for in-CPI validation. Mint is already
    // pinned via `address = USDC_MINT` on the Accounts struct so this
    // is strict defense-in-depth — but if a future refactor weakens
    // the binding, `transfer_checked` would still reject a mismatched
    // mint and the wrong decimals.
    let cpi_accounts = TransferChecked {
        from: ctx.accounts.agent_token_account.to_account_info(),
        mint: ctx.accounts.mint.to_account_info(),
        to: ctx.accounts.vault.to_account_info(),
        authority: ctx.accounts.agent.to_account_info(),
    };
    let cpi_ctx = CpiContext::new(ctx.accounts.token_program.to_account_info(), cpi_accounts);
    token::transfer_checked(cpi_ctx, amount, ctx.accounts.mint.decimals)?;

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
// `amount` is unused in any account constraint — only `service_id` is bound
// (PDA seeds). Renamed to `_amount` to silence the unused-instruction-arg
// noise without changing the IX serialization (positional, not by name).
#[instruction(_amount: u64, service_id: [u8; 32])]
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
    /// `init_if_needed` because the escrow PDA seeds include
    /// `service_id`, so different requests from the same agent need
    /// distinct vault ATAs — the first deposit for a given service_id
    /// creates the vault; if a prior escrow with the *same* service_id
    /// somehow leaves a vault around (it shouldn't — claim/refund close
    /// it), re-init is a safe no-op because Anchor checks
    /// `is_initialized` first. The ATA address is uniquely determined
    /// by `(mint, escrow_pda)` so an attacker can't substitute a
    /// different account. See `programs/escrow/AGENTS.md`
    /// "init_if_needed exception".
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
