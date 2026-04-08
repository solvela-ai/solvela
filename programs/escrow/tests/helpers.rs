#![allow(dead_code)]

use anchor_lang::solana_program::hash::hash;
use litesvm::LiteSVM;
use solana_sdk::{
    account::Account,
    instruction::{AccountMeta, Instruction},
    program_pack::Pack,
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

/// Program ID — must match declare_id!() in lib.rs
pub const PROGRAM_ID: Pubkey = solana_sdk::pubkey!("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU");

/// Mainnet USDC mint — must match USDC_MINT in lib.rs
pub const USDC_MINT: Pubkey = solana_sdk::pubkey!("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v");

/// Compiled program bytes
const PROGRAM_SO: &[u8] = include_bytes!("../target/deploy/rustyclawrouter_escrow.so");

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

pub fn find_escrow_pda(program_id: &Pubkey, agent: &Pubkey, service_id: &[u8; 32]) -> (Pubkey, u8) {
    Pubkey::find_program_address(
        &[b"escrow", agent.as_ref(), service_id],
        program_id,
    )
}

pub fn find_vault_ata(escrow_pda: &Pubkey, mint: &Pubkey) -> Pubkey {
    get_associated_token_address(escrow_pda, mint)
}

pub fn discriminator(ix_name: &str) -> [u8; 8] {
    let preimage = format!("global:{}", ix_name);
    let h = hash(preimage.as_bytes());
    let mut d = [0u8; 8];
    d.copy_from_slice(&h.to_bytes()[..8]);
    d
}

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
    svm.get_account(pubkey)
        .map(|a| a.lamports > 0)
        .unwrap_or(false)
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
