# LiteSVM Integration Tests for Escrow Program

**Date:** 2026-03-31
**Status:** Approved
**Scope:** Add 14 integration tests using LiteSVM 0.6.1 for the Anchor escrow program at `programs/escrow/`

## Context

The escrow program (`programs/escrow/`) has 3 instructions (deposit, claim, refund), 5 error types, and PDA-based vault management. Currently 6 unit tests exist in `tests/unit.rs` covering compile-time checks (struct sizes, PDA determinism). No integration tests exist that exercise the actual program logic on a simulated Solana runtime.

### Constraints
- Anchor 0.31.1, LiteSVM 0.6.1, solana-sdk 2.2
- `anchor-litesvm` crate is incompatible (requires anchor ^1.0, litesvm ^0.8)
- Must use raw LiteSVM with manual instruction building
- `anchor build` must produce `.so` before tests can run
- Program is NOT a workspace member (standalone Cargo.toml)

## File Structure

```
programs/escrow/tests/
├── unit.rs            # Existing — 6 compile-time tests (unchanged)
├── helpers.rs         # NEW — test harness module
└── integration.rs     # NEW — 14 LiteSVM integration tests
```

## Test Harness (`helpers.rs`)

### TestCtx Struct
```rust
struct TestCtx {
    svm: LiteSVM,
    program_id: Pubkey,
    agent: Keypair,
    provider: Keypair,
    usdc_mint: Pubkey,
    mint_authority: Keypair,
}
```

### Helper Functions

| Function | Purpose |
|----------|---------|
| `setup()` | Creates LiteSVM with `.with_spl_programs()`, loads escrow `.so` via `include_bytes!`, airdrops 10 SOL to agent + provider, injects USDC mint via `set_account` (6 decimals) |
| `inject_ata(svm, owner, mint, amount)` | Derives ATA address, injects token account with given USDC balance via `set_account` |
| `find_escrow_pda(program_id, agent, service_id)` | Wraps `Pubkey::find_program_address` with seeds `[b"escrow", agent, service_id]` |
| `find_vault_ata(escrow_pda, mint)` | Derives PDA-owned ATA for the vault |
| `discriminator(ix_name)` | `hash("global:{ix_name}").to_bytes()[..8]` |
| `build_deposit_ix(program_id, agent, provider, mint, amount, service_id, expiry_slot)` | Builds deposit instruction with discriminator, serialized args (amount u64 LE, service_id [u8;32], expiry_slot u64 LE), and account metas matching Deposit Accounts struct order |
| `build_claim_ix(program_id, escrow_pda, agent, provider, mint, vault, service_id, bump, actual_amount)` | Builds claim instruction with discriminator + actual_amount u64 LE, account metas matching Claim Accounts struct order. Note: service_id and bump are used only for PDA/vault address derivation, NOT serialized into instruction data. Only `discriminator("claim") ++ actual_amount: u64 LE` goes into instruction data. |
| `build_refund_ix(program_id, escrow_pda, agent, mint, vault, service_id, bump)` | Builds refund instruction with discriminator only (no args), account metas matching Refund Accounts struct order. Note: service_id and bump are used only for PDA/vault address derivation, NOT serialized into instruction data. Only `discriminator("refund")` goes into instruction data (no args). |
| `send_tx(svm, ixs, payer, signers)` | Wraps Transaction::new_signed_with_payer + send_transaction, returns Result |
| `read_escrow_account(svm, pda)` | Reads account data, skips 8-byte Anchor discriminator, deserializes Escrow struct fields |
| `read_token_balance(svm, ata)` | Reads SPL token account, returns amount field |
| `warp_and_refresh(svm, slot)` | Calls `warp_to_slot(slot)` then `expire_blockhash()` |

### Token Setup Pattern (set_account injection)
Instead of sending SPL token program transactions, inject token state directly via `svm.set_account()`:
- Mint: Serialize `spl_token::state::Mint` with `Pack::pack`, set owner to `TOKEN_PROGRAM_ID`
- ATA: Serialize `spl_token::state::Account` with `Pack::pack`, set owner to `TOKEN_PROGRAM_ID`, derive address via `get_associated_token_address`

### Instruction Arg Serialization
- Deposit: `discriminator("deposit") ++ amount: u64 LE ++ service_id: [u8;32] ++ expiry_slot: u64 LE`
- Claim: `discriminator("claim") ++ actual_amount: u64 LE`
- Refund: `discriminator("refund")` (no args)

Note: Verify exact arg order matches Anchor's serialization of the instruction handler parameters. Anchor serializes args in the order they appear in the function signature.

### Account Meta Ordering
Must exactly match the field order in each `#[derive(Accounts)]` struct:

**Deposit:** agent (signer, mut), provider (unchecked), mint, escrow (PDA, init), agent_token_account, vault (PDA ATA, init_if_needed), token_program, associated_token_program, system_program

**Claim:** escrow (PDA, mut, close=agent), agent, provider (signer, mut), mint, vault (PDA ATA, mut), provider_token_account (init_if_needed), agent_token_account, token_program, associated_token_program, system_program

**Refund:** escrow (PDA, mut, close=agent), agent (signer, mut), mint, vault (PDA ATA, mut), agent_token_account, token_program, associated_token_program, system_program

## Test Cases (`integration.rs`)

### Happy Path (5 tests)

| # | Test Name | Steps | Assertions |
|---|-----------|-------|------------|
| 1 | `test_deposit_creates_escrow` | Inject agent ATA with 1 USDC, send deposit(1_000_000, service_id, expiry=500) | Escrow PDA exists with correct agent/provider/mint/amount/service_id/expiry. Vault holds 1_000_000 tokens. Agent ATA balance decreased by 1_000_000. |
| 2 | `test_claim_full_amount` | Deposit 1 USDC → claim(1_000_000) | Provider ATA has 1_000_000. Agent ATA unchanged (no refund). Escrow PDA closed (account doesn't exist). Vault closed. |
| 3 | `test_claim_partial_with_refund` | Deposit 1 USDC → claim(600_000) | Provider ATA has 600_000. Agent ATA increased by 400_000. Escrow + vault closed. |
| 4 | `test_refund_after_expiry` | Deposit 1 USDC (expiry=100) → warp_to_slot(100) → refund | Agent ATA has full 1_000_000 back. Escrow + vault closed. |
| 5 | `test_multiple_escrows_same_agent` | Deposit 1 USDC (service_id_a) + deposit 2 USDC (service_id_b) → claim service_id_a | Service_a escrow closed, provider has 1 USDC. Service_b escrow still exists with 2 USDC. |

### Error Cases (9 tests)

| # | Test Name | Steps | Expected Error |
|---|-----------|-------|---------------|
| 6 | `test_deposit_zero_amount_fails` | deposit(0, ...) | `ZeroAmount` (custom error code) |
| 7 | `test_deposit_expiry_in_past_fails` | Warp to slot 100, deposit(..., expiry=50) | `InvalidExpiry` |
| 8 | `test_claim_exceeds_deposit_fails` | Deposit 1 USDC → claim(2_000_000) | `ClaimExceedsDeposit` |
| 9 | `test_claim_zero_amount_fails` | Deposit 1 USDC → claim(0) | `ZeroAmount` |
| 10 | `test_claim_wrong_provider_fails` | Deposit with provider_a → claim signed by provider_b | Anchor constraint error (signer mismatch) |
| 11 | `test_refund_before_expiry_fails` | Deposit (expiry=1000) at slot 0 → refund at slot 50 | `EscrowNotExpired` |
| 12 | `test_refund_wrong_agent_fails` | Deposit by agent_a → refund signed by agent_b | Anchor constraint error (signer mismatch) |
| 13 | `test_deposit_insufficient_balance_fails` | Inject agent ATA with 0.5 USDC → deposit(1_000_000) | SPL token insufficient funds error |
| 14 | `test_deposit_wrong_mint_fails` | Inject a non-USDC mint, attempt deposit with it | Anchor constraint error (address mismatch on mint) |

### Error Detection Pattern
LiteSVM's `send_transaction()` returns `Err(FailedTransactionMetadata)` on failure. For Anchor custom errors, the error code is embedded in the transaction logs. Check for:
- Custom error codes: `Error Code: <ErrorName>` in logs or `InstructionError(0, Custom(<code>))`
- Anchor error codes start at 6000 + enum variant index
- SPL token errors: `InstructionError(_, Custom(1))` for insufficient funds

## Prerequisites

1. `anchor build` must run first to produce `target/deploy/solvela_escrow.so`
2. Tests load `.so` via `include_bytes!` — compilation fails if file doesn't exist
3. Add to CI: `cd programs/escrow && anchor build && cargo test`

## Dependencies (already in Cargo.toml)

- `litesvm = "0.6.1"` (dev-dependency, already present)
- `solana-sdk = "2.2"` (dev-dependency, already present)

### Additional dev-dependencies needed
- `spl-token = "7"` — for `Mint` and `Account` Pack/unpack and constants
- `spl-associated-token-account = "5"` — for `get_associated_token_address`
- `solana-program-pack = "2.2"` — for the `Pack` trait (may be re-exported from spl-token)

Check exact compatible versions against solana-sdk 2.2 before adding.

## Success Criteria

- All 14 integration tests pass against the compiled `.so`
- Tests run in <5 seconds (LiteSVM is in-process, no validator)
- No changes to the escrow program source code
- Helpers are reusable for future test expansion
