<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# instructions

## Purpose
Per-instruction Anchor handlers. Each file defines the accounts struct (`#[derive(Accounts)]`) and the handler function. Dispatched from `../lib.rs`.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; re-exports each instruction handler and its `Accounts` struct |
| `deposit.rs` | `deposit` — agent deposits USDC into the escrow PDA; creates the account on first call |
| `claim.rs` | `claim` — provider claims up to `amount` USDC; enforces `actual_amount <= escrow.amount`; closes the account if fully drained |
| `refund.rs` | `refund` — agent refunds remaining USDC after `expiry_slot` is reached; closes the account |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Always validate the calling authority with `has_one` or `Signer<'info>`. Never accept `UncheckedAccount` for authority fields.
- `require!(actual_amount <= ctx.accounts.escrow.amount, EscrowError::ClaimExceedsDeposit);` is the canonical claim-amount guard.
- Use `close = refund_recipient` on the escrow account when draining it fully — prevents revival attacks.
- CPI to the token program with `CpiContext::new_with_signer` when transferring from the escrow PDA.

### Testing Requirements
```bash
cargo test --manifest-path programs/escrow/Cargo.toml
```
Each instruction gets at least: happy path, unauthorised caller, exceeds-deposit, pre-expiry refund attempt, duplicate-init attempt.

### Common Patterns
- Checked arithmetic: `escrow.amount = escrow.amount.checked_sub(amount).ok_or(…)?;`
- Duplicate-mutable-account guard: `require!(from.key() != to.key(), …);`
- Re-read account state after a CPI before relying on it.

## Dependencies

### Internal
- `super::state::Escrow`, `super::errors::EscrowError`.

### External
- `anchor-lang`, `anchor-spl::token`.

<!-- MANUAL: -->
