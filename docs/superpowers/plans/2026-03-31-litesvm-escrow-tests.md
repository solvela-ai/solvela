# LiteSVM Escrow Integration Tests — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add 14 LiteSVM integration tests for the Anchor escrow program (deposit, claim, refund) covering happy paths and error cases.

**Architecture:** Raw LiteSVM 0.6.1 with manual Anchor instruction building. Test harness in `helpers.rs` provides setup, token injection, instruction builders, and account readers. Integration tests in `integration.rs` use the harness. No `anchor-litesvm` (incompatible versions).

**Tech Stack:** LiteSVM 0.6.1, Anchor 0.31.1, solana-sdk 2.2, spl-token, spl-associated-token-account

**Spec:** `docs/superpowers/specs/2026-03-31-litesvm-escrow-integration-tests-design.md`

---

### Task 0: Prerequisites — Anchor Build + Dev Dependencies

**Files:**
- Modify: `programs/escrow/Cargo.toml`

- [ ] **Step 1: Add missing dev-dependencies to Cargo.toml**

Add after the existing `solana-sdk = "2.2"` line in `[dev-dependencies]`:

```toml
spl-token = "7"
spl-associated-token-account = "5"
```

- [ ] **Step 2: Run anchor build to produce the .so**

```bash
cd programs/escrow && anchor build
```

Expected: `target/deploy/rustyclawrouter_escrow.so` exists.

If anchor CLI is not installed or build fails due to environment issues, note the error and skip to Task 1 (helpers can be written without the .so; integration tests will fail to compile without it but that's expected).

- [ ] **Step 3: Verify existing tests still pass**

```bash
cd programs/escrow && cargo test --lib
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add programs/escrow/Cargo.toml
git commit -m "chore: add spl-token dev-dependencies for LiteSVM integration tests"
```

---

### Task 1: Test Harness — `helpers.rs`

**Files:**
- Create: `programs/escrow/tests/helpers.rs`

- [ ] **Step 1: Create the helpers module file**

Write `programs/escrow/tests/helpers.rs` with the full test harness. This includes:

**Constants:**
```rust
use anchor_lang::solana_program::hash::hash;
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_program,
    transaction::Transaction,
};
use spl_associated_token_account::get_associated_token_address;
use spl_token::{
    state::{Account as TokenAccount, Mint},
    ID as TOKEN_PROGRAM_ID,
};
use solana_sdk::program_pack::Pack;

/// Program ID — must match declare_id!() in lib.rs
pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");

/// Mainnet USDC mint — must match USDC_MINT in lib.rs
pub const USDC_MINT: Pubkey = solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

/// Compiled program bytes
const PROGRAM_SO: &[u8] = include_bytes!("../target/deploy/rustyclawrouter_escrow.so");
```

**TestCtx struct and setup:**
```rust
pub struct TestCtx {
    pub svm: LiteSVM,
    pub program_id: Pubkey,
    pub agent: Keypair,
    pub provider: Keypair,
    pub mint_authority: Keypair,
    pub usdc_mint: Pubkey,
}

pub fn setup() -> TestCtx {
    let mut svm = LiteSVM::new().with_spl_programs();
    svm.add_program(PROGRAM_ID, PROGRAM_SO);

    let agent = Keypair::new();
    let provider = Keypair::new();
    let mint_authority = Keypair::new();
    svm.airdrop(&agent.pubkey(), 10_000_000_000).unwrap();
    svm.airdrop(&provider.pubkey(), 10_000_000_000).unwrap();

    // Inject USDC mint via set_account (not via SPL token CPI)
    let mut mint_data = vec![0u8; Mint::LEN];
    Mint::pack(
        Mint {
            mint_authority: solana_sdk::program_option::COption::Some(mint_authority.pubkey()),
            supply: 100_000_000_000, // 100k USDC supply
            decimals: 6,
            is_initialized: true,
            freeze_authority: solana_sdk::program_option::COption::None,
        },
        &mut mint_data,
    )
    .unwrap();
    svm.set_account(
        USDC_MINT,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(Mint::LEN),
            data: mint_data,
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();

    TestCtx {
        svm,
        program_id: PROGRAM_ID,
        agent,
        provider,
        mint_authority,
        usdc_mint: USDC_MINT,
    }
}
```

**inject_ata helper:**
```rust
pub fn inject_ata(svm: &mut LiteSVM, owner: &Pubkey, mint: &Pubkey, amount: u64) -> Pubkey {
    let ata = get_associated_token_address(owner, mint);
    let mut data = vec![0u8; TokenAccount::LEN];
    TokenAccount::pack(
        TokenAccount {
            mint: *mint,
            owner: *owner,
            amount,
            delegate: solana_sdk::program_option::COption::None,
            state: spl_token::state::AccountState::Initialized,
            is_native: solana_sdk::program_option::COption::None,
            delegated_amount: 0,
            close_authority: solana_sdk::program_option::COption::None,
        },
        &mut data,
    )
    .unwrap();
    svm.set_account(
        ata,
        Account {
            lamports: svm.minimum_balance_for_rent_exemption(TokenAccount::LEN),
            data,
            owner: TOKEN_PROGRAM_ID,
            executable: false,
            rent_epoch: 0,
        },
    )
    .unwrap();
    ata
}
```

**PDA helpers:**
```rust
pub fn find_escrow_pda(program_id: &Pubkey, agent: &Pubkey, service_id: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"escrow", agent.as_ref(), service_id],
        program_id,
    )
}

pub fn find_vault_ata(escrow_pda: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(escrow_pda, mint)
}
```

**Discriminator helper:**
```rust
pub fn discriminator(ix_name: &str) -> [u8; 8] {
    let preimage = format!("global:{}", ix_name);
    let h = hash(preimage.as_bytes());
    let mut d = [0u8; 8];
    d.copy_from_slice(&h.to_bytes()[..8]);
    d
}
```

**Instruction builders:**
```rust
pub fn build_deposit_ix(
    program_id: &Pubkey,
    agent: &Pubkey,
    provider: &Pubkey,
    mint: &Pubkey,
    amount: u64,
    service_id: &[u8; 32],
    expiry_slot: u64,
) -> Instruction {
    let (escrow_pda, _) = find_escrow_pda(program_id, agent, service_id);
    let agent_ata = get_associated_token_address(agent, mint);
    let vault_ata = get_associated_token_address(&escrow_pda, mint);

    let mut data = discriminator("deposit").to_vec();
    data.extend_from_slice(&amount.to_le_bytes());
    data.extend_from_slice(service_id);
    data.extend_from_slice(&expiry_slot.to_le_bytes());

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(*agent, true),           // agent (signer, mut)
            AccountMeta::new_readonly(*provider, false), // provider (unchecked)
            AccountMeta::new_readonly(*mint, false),   // mint
            AccountMeta::new(escrow_pda, false),       // escrow (PDA, init)
            AccountMeta::new(agent_ata, false),        // agent_token_account
            AccountMeta::new(vault_ata, false),        // vault (init_if_needed)
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

pub fn build_claim_ix(
    program_id: &Pubkey,
    agent: &Pubkey,
    provider: &Pubkey,
    mint: &Pubkey,
    service_id: &[u8; 32],
    bump: u8,
    actual_amount: u64,
) -> Instruction {
    let (escrow_pda, _) = find_escrow_pda(program_id, agent, service_id);
    let vault_ata = get_associated_token_address(&escrow_pda, mint);
    let provider_ata = get_associated_token_address(provider, mint);
    let agent_ata = get_associated_token_address(agent, mint);
    let _ = bump; // used only for documentation; PDA bump is stored on-chain

    let mut data = discriminator("claim").to_vec();
    data.extend_from_slice(&actual_amount.to_le_bytes());

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(escrow_pda, false),       // escrow (PDA, mut, close=agent)
            AccountMeta::new(*agent, false),            // agent (SystemAccount, mut)
            AccountMeta::new(*provider, true),          // provider (signer, mut)
            AccountMeta::new_readonly(*mint, false),    // mint
            AccountMeta::new(vault_ata, false),         // vault (mut)
            AccountMeta::new(provider_ata, false),      // provider_token_account (init_if_needed)
            AccountMeta::new(agent_ata, false),         // agent_token_account (mut)
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}

pub fn build_refund_ix(
    program_id: &Pubkey,
    agent: &Pubkey,
    mint: &Pubkey,
    service_id: &[u8; 32],
    bump: u8,
) -> Instruction {
    let (escrow_pda, _) = find_escrow_pda(program_id, agent, service_id);
    let vault_ata = get_associated_token_address(&escrow_pda, mint);
    let agent_ata = get_associated_token_address(agent, mint);
    let _ = bump;

    let data = discriminator("refund").to_vec();

    Instruction {
        program_id: *program_id,
        accounts: vec![
            AccountMeta::new(escrow_pda, false),       // escrow (PDA, mut, close=agent)
            AccountMeta::new(*agent, true),             // agent (signer, mut)
            AccountMeta::new_readonly(*mint, false),    // mint
            AccountMeta::new(vault_ata, false),         // vault (mut)
            AccountMeta::new(agent_ata, false),         // agent_token_account (mut)
            AccountMeta::new_readonly(TOKEN_PROGRAM_ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    }
}
```

**Transaction and account reading helpers:**
```rust
pub fn send_tx(
    svm: &mut LiteSVM,
    ixs: &[Instruction],
    payer: &Keypair,
    signers: &[&Keypair],
) -> Result<litesvm::types::TransactionMetadata, litesvm::types::FailedTransactionMetadata> {
    let blockhash = svm.latest_blockhash();
    let tx = Transaction::new_signed_with_payer(ixs, Some(&payer.pubkey()), signers, blockhash);
    svm.send_transaction(tx)
}

pub fn read_token_balance(svm: &LiteSVM, ata: &Pubkey) -> Option<u64> {
    let account = svm.get_account(ata)?;
    let token_account = TokenAccount::unpack(&account.data).ok()?;
    Some(token_account.amount)
}

pub fn account_exists(svm: &LiteSVM, pubkey: &Pubkey) -> bool {
    svm.get_account(pubkey).is_some()
}

#[derive(Debug)]
pub struct EscrowData {
    pub agent: Pubkey,
    pub provider: Pubkey,
    pub mint: Pubkey,
    pub amount: u64,
    pub service_id: [u8; 32],
    pub expiry_slot: u64,
    pub bump: u8,
}

pub fn read_escrow_account(svm: &LiteSVM, pda: &Pubkey) -> Option<EscrowData> {
    let account = svm.get_account(pda)?;
    let data = &account.data;
    if data.len() < 8 + 145 {
        return None;
    }
    let d = &data[8..]; // skip Anchor discriminator
    Some(EscrowData {
        agent: Pubkey::try_from(&d[0..32]).unwrap(),
        provider: Pubkey::try_from(&d[32..64]).unwrap(),
        mint: Pubkey::try_from(&d[64..96]).unwrap(),
        amount: u64::from_le_bytes(d[96..104].try_into().unwrap()),
        service_id: d[104..136].try_into().unwrap(),
        expiry_slot: u64::from_le_bytes(d[136..144].try_into().unwrap()),
        bump: d[144],
    })
}

pub fn warp_and_refresh(svm: &mut LiteSVM, slot: u64) {
    svm.warp_to_slot(slot);
    svm.expire_blockhash();
}

/// Helper: deposit and return (escrow_pda, bump)
pub fn deposit_helper(
    ctx: &mut TestCtx,
    service_id: &[u8; 32],
    amount: u64,
    expiry_slot: u64,
) -> (Pubkey, u8) {
    let (escrow_pda, bump) = find_escrow_pda(&ctx.program_id, &ctx.agent.pubkey(), service_id);
    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        amount,
        service_id,
        expiry_slot,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]).expect("deposit should succeed");
    (escrow_pda, bump)
}
```

- [ ] **Step 2: Verify the file compiles (may fail without .so — that's expected)**

```bash
cd programs/escrow && cargo check --tests 2>&1 | head -20
```

If it fails with "include_bytes: file not found", that's expected — the .so hasn't been built yet. The file structure is correct.

- [ ] **Step 3: Commit**

```bash
git add programs/escrow/tests/helpers.rs
git commit -m "test: add LiteSVM test harness for escrow integration tests"
```

---

### Task 2: Happy Path Tests (1-5)

**Files:**
- Create: `programs/escrow/tests/integration.rs`

- [ ] **Step 1: Create integration.rs with the module declaration and first 5 tests**

Write `programs/escrow/tests/integration.rs`:

```rust
mod helpers;

use helpers::*;
use solana_sdk::signer::Signer;
use spl_associated_token_account::get_associated_token_address;

// ─── Happy Path Tests ──────────────────────────────────────────────────────

#[test]
fn test_deposit_creates_escrow() {
    let mut ctx = setup();
    let service_id = [1u8; 32];
    let amount = 1_000_000u64; // 1 USDC
    let expiry_slot = 500u64;

    // Fund agent with USDC
    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);

    let (escrow_pda, _bump) = deposit_helper(&mut ctx, &service_id, amount, expiry_slot);

    // Verify escrow PDA exists
    assert!(account_exists(&ctx.svm, &escrow_pda), "escrow PDA should exist");

    // Verify vault holds the tokens
    let vault = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &vault), Some(amount));

    // Verify agent ATA was debited
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &agent_ata), Some(0));

    // Verify escrow fields
    let escrow = read_escrow_account(&ctx.svm, &escrow_pda).expect("escrow should be readable");
    assert_eq!(escrow.agent, ctx.agent.pubkey());
    assert_eq!(escrow.provider, ctx.provider.pubkey());
    assert_eq!(escrow.mint, ctx.usdc_mint);
    assert_eq!(escrow.amount, amount);
    assert_eq!(escrow.service_id, service_id);
    assert_eq!(escrow.expiry_slot, expiry_slot);
}

#[test]
fn test_claim_full_amount() {
    let mut ctx = setup();
    let service_id = [2u8; 32];
    let amount = 1_000_000u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);
    let (escrow_pda, bump) = deposit_helper(&mut ctx, &service_id, amount, 500);

    // Claim full amount
    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        amount,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]).expect("claim should succeed");

    // Provider got all tokens
    let provider_ata = get_associated_token_address(&ctx.provider.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &provider_ata), Some(amount));

    // Escrow PDA closed
    assert!(!account_exists(&ctx.svm, &escrow_pda), "escrow should be closed");

    // Agent ATA unchanged (no refund)
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &agent_ata), Some(0), "agent should have no refund");

    // Vault closed
    let vault = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    assert!(!account_exists(&ctx.svm, &vault), "vault should be closed");
}

#[test]
fn test_claim_partial_with_refund() {
    let mut ctx = setup();
    let service_id = [3u8; 32];
    let deposit_amount = 1_000_000u64;
    let claim_amount = 600_000u64;
    let refund_amount = deposit_amount - claim_amount;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, deposit_amount);
    let (escrow_pda, bump) = deposit_helper(&mut ctx, &service_id, deposit_amount, 500);

    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        claim_amount,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]).expect("partial claim should succeed");

    // Provider got claim amount
    let provider_ata = get_associated_token_address(&ctx.provider.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &provider_ata), Some(claim_amount));

    // Agent got refund
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &agent_ata), Some(refund_amount));

    // Escrow closed
    assert!(!account_exists(&ctx.svm, &escrow_pda), "escrow should be closed");

    // Vault closed
    let vault = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    assert!(!account_exists(&ctx.svm, &vault), "vault should be closed");
}

#[test]
fn test_refund_after_expiry() {
    let mut ctx = setup();
    let service_id = [4u8; 32];
    let amount = 1_000_000u64;
    let expiry_slot = 100u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);
    let (escrow_pda, bump) = deposit_helper(&mut ctx, &service_id, amount, expiry_slot);

    // Warp past expiry
    warp_and_refresh(&mut ctx.svm, expiry_slot);

    let ix = build_refund_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]).expect("refund should succeed");

    // Agent got full refund
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &agent_ata), Some(amount));

    // Escrow closed
    assert!(!account_exists(&ctx.svm, &escrow_pda), "escrow should be closed");

    // Vault closed
    let vault = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    assert!(!account_exists(&ctx.svm, &vault), "vault should be closed");
}

#[test]
fn test_multiple_escrows_same_agent() {
    let mut ctx = setup();
    let service_id_a = [10u8; 32];
    let service_id_b = [11u8; 32];
    let amount_a = 1_000_000u64;
    let amount_b = 2_000_000u64;

    // Fund agent with enough for both deposits
    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount_a + amount_b);

    // Two separate deposits
    let (_escrow_a, bump_a) = deposit_helper(&mut ctx, &service_id_a, amount_a, 500);
    let (escrow_b, _bump_b) = deposit_helper(&mut ctx, &service_id_b, amount_b, 500);

    // Claim only escrow A
    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id_a,
        bump_a,
        amount_a,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]).expect("claim A should succeed");

    // Escrow A closed
    let (pda_a, _) = find_escrow_pda(&ctx.program_id, &ctx.agent.pubkey(), &service_id_a);
    assert!(!account_exists(&ctx.svm, &pda_a), "escrow A should be closed");

    // Escrow B still exists with funds
    assert!(account_exists(&ctx.svm, &escrow_b), "escrow B should still exist");
    let vault_b = find_vault_ata(&escrow_b, &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &vault_b), Some(amount_b));
}
```

- [ ] **Step 2: Verify tests compile (requires anchor build .so)**

```bash
cd programs/escrow && cargo test --test integration -- --list 2>&1
```

Expected: Lists 5 test names.

- [ ] **Step 3: Run happy path tests**

```bash
cd programs/escrow && cargo test --test integration 2>&1
```

Expected: 5 tests pass.

- [ ] **Step 4: Commit**

```bash
git add programs/escrow/tests/integration.rs
git commit -m "test: add 5 happy-path LiteSVM integration tests for escrow"
```

---

### Task 3: Error Case Tests (6-14)

**Files:**
- Modify: `programs/escrow/tests/integration.rs`

- [ ] **Step 1: Add the 9 error case tests to integration.rs**

Append to `programs/escrow/tests/integration.rs`:

```rust
// ─── Error Case Tests ──────────────────────────────────────────────────────

#[test]
fn test_deposit_zero_amount_fails() {
    let mut ctx = setup();
    let service_id = [20u8; 32];

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);

    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        0, // zero amount
        &service_id,
        500,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(result.is_err(), "deposit with zero amount should fail");
}

#[test]
fn test_deposit_expiry_in_past_fails() {
    let mut ctx = setup();
    let service_id = [21u8; 32];

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);

    // Warp forward so expiry_slot=50 is in the past
    warp_and_refresh(&mut ctx.svm, 100);

    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        1_000_000,
        &service_id,
        50, // in the past
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(result.is_err(), "deposit with past expiry should fail");
}

#[test]
fn test_claim_exceeds_deposit_fails() {
    let mut ctx = setup();
    let service_id = [22u8; 32];

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 500);

    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        2_000_000, // exceeds deposit
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]);
    assert!(result.is_err(), "claim exceeding deposit should fail");
}

#[test]
fn test_claim_zero_amount_fails() {
    let mut ctx = setup();
    let service_id = [23u8; 32];

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 500);

    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        0, // zero claim
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]);
    assert!(result.is_err(), "claim of zero should fail");
}

#[test]
fn test_claim_wrong_provider_fails() {
    let mut ctx = setup();
    let service_id = [24u8; 32];
    let wrong_provider = solana_sdk::signature::Keypair::new();
    ctx.svm.airdrop(&wrong_provider.pubkey(), 10_000_000_000).unwrap();

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 500);

    // Build claim with the correct agent/provider addresses (for PDA derivation)
    // but sign with wrong_provider
    let (escrow_pda, _) = find_escrow_pda(&ctx.program_id, &ctx.agent.pubkey(), &service_id);
    let vault_ata = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    let wrong_provider_ata = get_associated_token_address(&wrong_provider.pubkey(), &ctx.usdc_mint);

    let mut data = discriminator("claim").to_vec();
    data.extend_from_slice(&1_000_000u64.to_le_bytes());

    let ix = solana_sdk::instruction::Instruction {
        program_id: ctx.program_id,
        accounts: vec![
            AccountMeta::new(escrow_pda, false),
            AccountMeta::new(ctx.agent.pubkey(), false),
            AccountMeta::new(wrong_provider.pubkey(), true), // wrong provider as signer
            AccountMeta::new_readonly(ctx.usdc_mint, false),
            AccountMeta::new(vault_ata, false),
            AccountMeta::new(wrong_provider_ata, false),
            AccountMeta::new(agent_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    };
    let result = send_tx(&mut ctx.svm, &[ix], &wrong_provider, &[&wrong_provider]);
    assert!(result.is_err(), "claim by wrong provider should fail");
}

#[test]
fn test_refund_before_expiry_fails() {
    let mut ctx = setup();
    let service_id = [25u8; 32];

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 1000);

    // Don't warp — still at slot ~0, expiry is 1000
    let ix = build_refund_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(result.is_err(), "refund before expiry should fail");
}

#[test]
fn test_refund_wrong_agent_fails() {
    let mut ctx = setup();
    let service_id = [26u8; 32];
    let wrong_agent = solana_sdk::signature::Keypair::new();
    ctx.svm.airdrop(&wrong_agent.pubkey(), 10_000_000_000).unwrap();

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 100);

    warp_and_refresh(&mut ctx.svm, 100);

    // Build refund with correct PDA derivation but wrong signer
    let (escrow_pda, _) = find_escrow_pda(&ctx.program_id, &ctx.agent.pubkey(), &service_id);
    let vault_ata = find_vault_ata(&escrow_pda, &ctx.usdc_mint);
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);

    let data = discriminator("refund").to_vec();

    let ix = solana_sdk::instruction::Instruction {
        program_id: ctx.program_id,
        accounts: vec![
            AccountMeta::new(escrow_pda, false),
            AccountMeta::new(wrong_agent.pubkey(), true), // wrong agent as signer
            AccountMeta::new_readonly(ctx.usdc_mint, false),
            AccountMeta::new(vault_ata, false),
            AccountMeta::new(agent_ata, false),
            AccountMeta::new_readonly(spl_token::ID, false),
            AccountMeta::new_readonly(spl_associated_token_account::ID, false),
            AccountMeta::new_readonly(system_program::ID, false),
        ],
        data,
    };
    let result = send_tx(&mut ctx.svm, &[ix], &wrong_agent, &[&wrong_agent]);
    assert!(result.is_err(), "refund by wrong agent should fail");
}

#[test]
fn test_deposit_insufficient_balance_fails() {
    let mut ctx = setup();
    let service_id = [27u8; 32];

    // Fund with only 0.5 USDC but try to deposit 1 USDC
    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 500_000);

    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        1_000_000, // more than balance
        &service_id,
        500,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(result.is_err(), "deposit exceeding balance should fail");
}

#[test]
fn test_deposit_wrong_mint_fails() {
    let mut ctx = setup();
    let service_id = [28u8; 32];

    // Create a fake mint (not USDC)
    let fake_mint = solana_sdk::pubkey::Pubkey::new_unique();
    let mut mint_data = vec![0u8; spl_token::state::Mint::LEN];
    spl_token::state::Mint::pack(
        spl_token::state::Mint {
            mint_authority: solana_sdk::program_option::COption::Some(ctx.agent.pubkey()),
            supply: 100_000_000,
            decimals: 6,
            is_initialized: true,
            freeze_authority: solana_sdk::program_option::COption::None,
        },
        &mut mint_data,
    )
    .unwrap();
    ctx.svm
        .set_account(
            fake_mint,
            solana_sdk::account::Account {
                lamports: ctx.svm.minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN),
                data: mint_data,
                owner: spl_token::ID,
                executable: false,
                rent_epoch: 0,
            },
        )
        .unwrap();

    // Inject ATA for the fake mint
    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &fake_mint, 1_000_000);

    // Build deposit with fake mint instead of USDC
    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &fake_mint, // wrong mint
        1_000_000,
        &service_id,
        500,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(result.is_err(), "deposit with wrong mint should fail");
}
```

- [ ] **Step 2: Run all 14 integration tests**

```bash
cd programs/escrow && cargo test --test integration 2>&1
```

Expected: 14 tests pass.

- [ ] **Step 3: Verify existing unit tests still pass**

```bash
cd programs/escrow && cargo test --test unit 2>&1
```

Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add programs/escrow/tests/integration.rs
git commit -m "test: add 9 error-case LiteSVM integration tests for escrow"
```

---

### Task 4: Final Verification

- [ ] **Step 1: Run all escrow tests together**

```bash
cd programs/escrow && cargo test 2>&1
```

Expected: 20 tests pass (6 unit + 14 integration).

- [ ] **Step 2: Verify test execution time**

Expected: <5 seconds total (LiteSVM is in-process).

- [ ] **Step 3: Verify no changes to program source**

```bash
git diff programs/escrow/src/
```

Expected: No changes.

- [ ] **Step 4: Final commit if any cleanup needed**

```bash
git status
```

If clean, no commit needed. If any formatting or cleanup, commit with:
```bash
git commit -m "test: finalize LiteSVM escrow integration test suite (14 tests)"
```
