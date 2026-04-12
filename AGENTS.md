# AGENTS.md — Solvela

Guidelines for AI coding agents operating in this repository.

**All build commands, test commands, code style, architecture, and architectural rules are in `CLAUDE.md`.** This file contains only Solana/Anchor/x402 reference material that is too detailed for CLAUDE.md.

---

## Solana / Anchor / x402 Development

This section is inlined from the `solana-dev` skill so all agents have access without
a manual skill invocation. For the full skill (IDL codegen, Pinocchio, Surfpool,
confidential transfers) invoke the `solana-dev` skill.

### Anchor Program Patterns

```rust
// Account sizing — always use InitSpace
#[account]
#[derive(InitSpace)]
pub struct Escrow {
    pub agent: Pubkey,        // 32
    pub provider: Pubkey,     // 32
    pub mint: Pubkey,         // 32
    pub amount: u64,          // 8
    pub service_id: [u8; 32], // 32
    pub expiry_slot: u64,     // 8
    pub bump: u8,             // 1
}
// space = 8 + Escrow::INIT_SPACE  (8-byte discriminator prefix)

// PDA derivation (this project's convention)
#[account(
    seeds = [b"escrow", agent.key().as_ref(), &service_id],
    bump,
)]
pub escrow: Account<'info, Escrow>,

// CPI with PDA signer
let seeds = &[b"escrow".as_ref(), agent.key().as_ref(), &service_id, &[bump]];
let signer = &[&seeds[..]];
let cpi_ctx = CpiContext::new_with_signer(token_program, cpi_accounts, signer);
token::transfer(cpi_ctx, amount)?;

// Error handling
#[error_code]
pub enum EscrowError {
    #[msg("Escrow has not yet expired")]
    NotExpired,
    #[msg("Claim amount exceeds deposited amount")]
    ClaimExceedsDeposit,
}
require!(actual_amount <= ctx.accounts.escrow.amount, EscrowError::ClaimExceedsDeposit);
```

### LiteSVM Testing (preferred for escrow unit tests)

```rust
use litesvm::LiteSVM;
use solana_sdk::{signature::Keypair, transaction::Transaction};

#[test]
fn test_deposit_instruction() {
    let mut svm = LiteSVM::new();
    let program_id = /* your program ID */;
    svm.add_program_from_file(program_id, "target/deploy/escrow.so");

    let payer = Keypair::new();
    svm.airdrop(&payer.pubkey(), 1_000_000_000).unwrap();

    // Warp clock for expiry tests
    svm.warp_to_slot(1000);

    let tx = Transaction::new_signed_with_payer(
        &[/* deposit instruction */],
        Some(&payer.pubkey()),
        &[&payer],
        svm.latest_blockhash(),
    );
    assert!(svm.send_transaction(tx).is_ok());
}
```

### SPL Token / x402 Payment Verification

```rust
// Token program IDs used in solana.rs
const TOKEN_PROGRAM_ID: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
const TOKEN_2022_PROGRAM_ID: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";

// Instruction discriminator: 12 = TransferChecked, 3 = Transfer
// Verify: program_id, destination ATA, amount >= required price, mint matches

// x402 payment flow:
// 1. Gateway returns 402 with PaymentRequired (price in atomic USDC units, 6 decimals)
// 2. Agent builds pre-signed SPL TransferChecked tx, base64-encodes into PAYMENT-SIGNATURE header
// 3. Gateway decodes → verifies → settles → proxies to LLM provider → returns 200
```

### Anchor Security Checklist (apply to escrow program)

- [ ] Use typed `Account<'info, T>` — never `UncheckedAccount` without explicit owner check
- [ ] Every authority field validated with `has_one` or explicit `Signer<'info>`
- [ ] PDA seeds include user-specific key — never shared across users
- [ ] No `init_if_needed` — use `init` to prevent reinitialization attacks
- [ ] CPIs use `Program<'info, Token>` — never accept arbitrary program accounts
- [ ] Checked arithmetic throughout: `checked_add`, `checked_sub`, `checked_mul`
- [ ] Account closure uses Anchor `close =` constraint — prevents revival attacks
- [ ] Duplicate mutable account check: `require!(from.key() != to.key(), ...)`
- [ ] After CPIs, re-read account state — do not rely on cached values

### Solana Constants (this project)

| Constant | Value |
|----------|-------|
| USDC mint (mainnet) | `EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v` |
| USDC decimals | 6 (1 USDC = 1_000_000 atomic units) |
| Platform fee | 5% added on top of provider cost |
| x402 version | 2 |
| Max payment timeout | 300 seconds |
| Solana network ID | `solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp` (mainnet) |
