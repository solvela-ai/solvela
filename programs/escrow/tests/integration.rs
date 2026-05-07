#![cfg(feature = "sbf")]

mod helpers;

use helpers::*;
use solana_sdk::{instruction::AccountMeta, program_pack::Pack, signer::Signer};
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
    assert!(
        account_exists(&ctx.svm, &escrow_pda),
        "escrow PDA should exist"
    );

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
    assert!(
        !account_exists(&ctx.svm, &escrow_pda),
        "escrow should be closed"
    );

    // Agent ATA unchanged (no refund)
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(
        read_token_balance(&ctx.svm, &agent_ata),
        Some(0),
        "agent should have no refund"
    );

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

    inject_ata(
        &mut ctx.svm,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        deposit_amount,
    );
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
    send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider])
        .expect("partial claim should succeed");

    // Provider got claim amount
    let provider_ata = get_associated_token_address(&ctx.provider.pubkey(), &ctx.usdc_mint);
    assert_eq!(
        read_token_balance(&ctx.svm, &provider_ata),
        Some(claim_amount)
    );

    // Agent got refund
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    assert_eq!(
        read_token_balance(&ctx.svm, &agent_ata),
        Some(refund_amount)
    );

    // Escrow closed
    assert!(
        !account_exists(&ctx.svm, &escrow_pda),
        "escrow should be closed"
    );

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
    assert!(
        !account_exists(&ctx.svm, &escrow_pda),
        "escrow should be closed"
    );

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
    inject_ata(
        &mut ctx.svm,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        amount_a + amount_b,
    );

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
    assert!(
        !account_exists(&ctx.svm, &pda_a),
        "escrow A should be closed"
    );

    // Escrow B still exists with funds
    assert!(
        account_exists(&ctx.svm, &escrow_b),
        "escrow B should still exist"
    );
    let vault_b = find_vault_ata(&escrow_b, &ctx.usdc_mint);
    assert_eq!(read_token_balance(&ctx.svm, &vault_b), Some(amount_b));
}

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
    ctx.svm
        .airdrop(&wrong_provider.pubkey(), 10_000_000_000)
        .unwrap();

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, _bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 500);

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
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
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
    ctx.svm
        .airdrop(&wrong_agent.pubkey(), 10_000_000_000)
        .unwrap();

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, 1_000_000);
    let (_escrow, _bump) = deposit_helper(&mut ctx, &service_id, 1_000_000, 100);

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
            AccountMeta::new_readonly(solana_sdk::system_program::ID, false),
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
                lamports: ctx
                    .svm
                    .minimum_balance_for_rent_exemption(spl_token::state::Mint::LEN),
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

// ─── Hardening regressions (post-review) ──────────────────────────────────
//
// These tests cover the failure modes flagged by the 2026-05-07 review:
//   - claim must be gated on slot < expiry_slot (race with refund)
//   - refund must survive a closed agent ATA (init_if_needed)
//   - claim's refund leg must do the same
//   - deposit must reject self-provider and excessive expiry

#[test]
fn test_claim_after_expiry_fails() {
    let mut ctx = setup();
    let service_id = [40u8; 32];
    let amount = 1_000_000u64;
    let expiry_slot = 100u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);
    let (_pda, bump) = deposit_helper(&mut ctx, &service_id, amount, expiry_slot);

    // Warp to the expiry boundary itself — claim must be rejected here, not
    // just after. The guard is `slot < expiry_slot` (strict inequality).
    warp_and_refresh(&mut ctx.svm, expiry_slot);

    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        amount,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider]);
    assert!(
        result.is_err(),
        "claim at expiry boundary must fail (EscrowExpired)"
    );

    // And a slot beyond expiry, to be belt-and-braces.
    warp_and_refresh(&mut ctx.svm, expiry_slot + 100);
    let ix2 = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        amount,
    );
    let result2 = send_tx(&mut ctx.svm, &[ix2], &ctx.provider, &[&ctx.provider]);
    assert!(
        result2.is_err(),
        "claim past expiry must fail (EscrowExpired)"
    );
}

#[test]
fn test_refund_after_agent_ata_closed_succeeds() {
    let mut ctx = setup();
    let service_id = [41u8; 32];
    let amount = 1_000_000u64;
    let expiry_slot = 100u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);
    let (_pda, bump) = deposit_helper(&mut ctx, &service_id, amount, expiry_slot);

    // Close the agent's ATA between deposit and refund — normal post-deposit
    // cleanup move that reclaims rent. Pre-fix this would brick the funds in
    // the vault PDA forever; with init_if_needed the refund instruction
    // recreates the ATA and refunds normally.
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    let close_ix = spl_token::instruction::close_account(
        &spl_token::ID,
        &agent_ata,
        &ctx.agent.pubkey(),
        &ctx.agent.pubkey(),
        &[],
    )
    .expect("build close_account ix");
    send_tx(&mut ctx.svm, &[close_ix], &ctx.agent, &[&ctx.agent])
        .expect("close empty agent ATA should succeed");
    assert!(
        !account_exists(&ctx.svm, &agent_ata),
        "agent ATA should be closed before refund"
    );

    // Warp to expiry and refund.
    warp_and_refresh(&mut ctx.svm, expiry_slot + 1);
    let ix = build_refund_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent])
        .expect("refund should succeed even after agent ATA was closed");

    // ATA should be re-created and hold the refunded amount.
    assert_eq!(
        read_token_balance(&ctx.svm, &agent_ata),
        Some(amount),
        "agent ATA should be re-created with the full refund"
    );
}

#[test]
fn test_claim_after_agent_ata_closed_succeeds() {
    let mut ctx = setup();
    let service_id = [42u8; 32];
    let deposit_amount = 1_000_000u64;
    let claim_amount = 600_000u64; // partial — exercises the refund leg

    inject_ata(
        &mut ctx.svm,
        &ctx.agent.pubkey(),
        &ctx.usdc_mint,
        deposit_amount,
    );
    let (_pda, bump) = deposit_helper(&mut ctx, &service_id, deposit_amount, 500);

    // Close the agent's ATA after deposit. Pre-fix this would brick the
    // claim's refund leg (and therefore the entire claim) on a partial
    // claim. With init_if_needed the provider pays for the recreation.
    let agent_ata = get_associated_token_address(&ctx.agent.pubkey(), &ctx.usdc_mint);
    let close_ix = spl_token::instruction::close_account(
        &spl_token::ID,
        &agent_ata,
        &ctx.agent.pubkey(),
        &ctx.agent.pubkey(),
        &[],
    )
    .expect("build close_account ix");
    send_tx(&mut ctx.svm, &[close_ix], &ctx.agent, &[&ctx.agent])
        .expect("close empty agent ATA should succeed");
    assert!(
        !account_exists(&ctx.svm, &agent_ata),
        "agent ATA should be closed before claim"
    );

    let ix = build_claim_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        &service_id,
        bump,
        claim_amount,
    );
    send_tx(&mut ctx.svm, &[ix], &ctx.provider, &[&ctx.provider])
        .expect("claim should succeed even after agent ATA was closed");

    // Provider received their portion; agent ATA was re-created with the
    // refund (deposit_amount - claim_amount).
    let provider_ata = get_associated_token_address(&ctx.provider.pubkey(), &ctx.usdc_mint);
    assert_eq!(
        read_token_balance(&ctx.svm, &provider_ata),
        Some(claim_amount),
        "provider should receive the claim amount"
    );
    assert_eq!(
        read_token_balance(&ctx.svm, &agent_ata),
        Some(deposit_amount - claim_amount),
        "agent ATA should be re-created with the refund remainder"
    );
}

#[test]
fn test_deposit_with_self_provider_rejected() {
    let mut ctx = setup();
    let service_id = [43u8; 32];
    let amount = 1_000_000u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);

    // Build a deposit IX where provider == agent. Pre-fix this would create
    // an escrow that only the agent (acting as provider) could claim, which
    // combined with the no-claim-expiry-guard bug would brick refund.
    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.agent.pubkey(), // provider == agent
        &ctx.usdc_mint,
        amount,
        &service_id,
        500,
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(
        result.is_err(),
        "deposit with provider == agent must fail (InvalidProvider)"
    );
}

#[test]
fn test_deposit_with_excessive_expiry_rejected() {
    let mut ctx = setup();
    let service_id = [44u8; 32];
    let amount = 1_000_000u64;

    inject_ata(&mut ctx.svm, &ctx.agent.pubkey(), &ctx.usdc_mint, amount);

    // MAX_ESCROW_SLOTS in lib.rs is 216_000. Anything beyond `now + that`
    // must be rejected so a buggy/adversarial client can't lock funds for
    // years (and combined with the no-expiry-claim-guard bug, exploit it).
    let ix = build_deposit_ix(
        &ctx.program_id,
        &ctx.agent.pubkey(),
        &ctx.provider.pubkey(),
        &ctx.usdc_mint,
        amount,
        &service_id,
        u64::MAX, // adversarial maximum
    );
    let result = send_tx(&mut ctx.svm, &[ix], &ctx.agent, &[&ctx.agent]);
    assert!(
        result.is_err(),
        "deposit with excessive expiry_slot must fail (ExpiryTooFar)"
    );
}
