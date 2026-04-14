# Escrow Client SDK Support

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable CLI and all SDKs (Python, TypeScript, Go) to pay via the escrow payment scheme, so agents get deposit protection with automatic refund on timeout.

**Architecture:** The escrow program is already deployed on mainnet and the gateway already verifies escrow deposits and fires claims. The protocol types (`EscrowPayload`, `PayloadData::Escrow`) already exist in the `rustyclaw-protocol` crate. This plan adds client-side deposit transaction building and scheme selection to each SDK. The x402 crate already has PDA derivation and transaction building utilities (used by the escrow claimer) — we export these for reuse by the CLI's deposit builder.

**Tech Stack:** Rust (CLI + x402), Python (solders/solana-py), TypeScript (@solana/web3.js), Go (types only)

---

## File Structure

### x402 crate (shared Rust protocol library)
- **Modify:** `crates/x402/src/escrow/pda.rs` — Make helpers `pub` (currently `pub(crate)`)
- **Modify:** `crates/x402/src/escrow/mod.rs` — Re-export public PDA helpers
- **Create:** `crates/x402/src/escrow/deposit.rs` — Deposit transaction builder (mirrors claimer pattern)

### CLI (Rust binary)
- **Modify:** `crates/cli/src/commands/chat.rs` — Scheme selection + escrow deposit flow
- **Modify:** `crates/cli/src/commands/solana_tx.rs` — Add `build_escrow_deposit` that calls x402 deposit builder

### Python SDK
- **Modify:** `sdks/python/rustyclawrouter/types.py` — Add `escrow_program_id` to `PaymentAccept`
- **Modify:** `sdks/python/rustyclawrouter/x402.py` — Add escrow deposit builder + scheme selection
- **Modify:** `sdks/python/rustyclawrouter/client.py` — Prefer escrow scheme when available
- **Modify:** `sdks/python/tests/test_x402.py` — Escrow payment tests

### TypeScript SDK
- **Modify:** `sdks/typescript/src/types.ts` — Add `escrow_program_id` to `PaymentAccept`
- **Modify:** `sdks/typescript/src/x402.ts` — Add escrow deposit builder + scheme selection
- **Modify:** `sdks/typescript/src/client.ts` — Prefer escrow scheme when available
- **Modify:** `sdks/typescript/tests/x402.test.ts` — Escrow payment tests

### Go SDK
- **Modify:** `sdks/go/types.go` — Add `EscrowProgramID` to `PaymentAccept`
- **Modify:** `sdks/go/x402.go` — Add escrow payload structure + scheme selection
- **Modify:** `sdks/go/x402_test.go` — Escrow payload tests

---

## Escrow Deposit Flow (all clients)

Every client follows this sequence when it selects the "escrow" scheme from a 402 response:

1. **Generate `service_id`** — SHA-256 of (serialized request body + 8 random bytes) to produce a unique 32-byte correlation ID. The random nonce prevents PDA collisions when an agent sends identical requests before the first escrow is claimed/refunded.
2. **Compute `expiry_slot`** — Fetch current slot from RPC, add `max_timeout_seconds / 0.4` (slot duration ~400ms). Round up for safety margin.
3. **Derive escrow PDA** — `find_program_address([b"escrow", agent_pubkey, service_id], escrow_program_id)`
4. **Derive ATAs** — Agent ATA and vault ATA (escrow PDA's ATA for USDC mint)
5. **Build deposit instruction** — Anchor discriminator `sha256("global:deposit")[..8]` + `amount(u64 LE)` + `service_id([u8;32])` + `expiry_slot(u64 LE)`. Nine accounts: agent, provider, mint, escrow, agent_ata, vault, token_program, ata_program, system_program.
6. **Build + sign legacy transaction** — Single instruction, agent signs.
7. **Encode payload** — `PayloadData::Escrow(EscrowPayload { deposit_tx, service_id, agent_pubkey })`

---

### Task 1: Export x402 Escrow PDA Helpers

**Context:** The x402 crate has PDA derivation, ATA derivation, anchor discriminator, and base58 pubkey decoding in `escrow/pda.rs` — but all are `pub(crate)`. The CLI and future Rust clients need these to build deposit transactions. The escrow claimer already uses them internally.

**Files:**
- Modify: `crates/x402/src/escrow/pda.rs:19-83` — Change visibility from `pub(crate)` to `pub`
- Modify: `crates/x402/src/escrow/mod.rs` — Add `pub mod pda;` or re-export key functions

- [ ] **Step 1: Write the test**

Add a test in `crates/x402/src/escrow/pda.rs` that uses the public API from outside the module (to verify exports work). Actually, the existing tests already validate the functions. The real test is that `crates/cli` can import them. Write a compile-time test in CLI:

```rust
// In crates/cli/src/commands/solana_tx.rs, add at bottom of test module:
#[test]
fn test_x402_escrow_pda_exports_accessible() {
    // Verify that x402 escrow PDA helpers are publicly accessible
    let program_id = x402::escrow::pda::decode_bs58_pubkey(
        "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
    ).unwrap();
    let agent = [1u8; 32];
    let service_id = [2u8; 32];
    let result = x402::escrow::pda::find_program_address(
        &[b"escrow", &agent, &service_id],
        &program_id,
    );
    assert!(result.is_some());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p rustyclawrouter-cli test_x402_escrow_pda_exports_accessible`
Expected: FAIL — compile error, `pda` module or functions not public.

- [ ] **Step 3: Make PDA helpers public**

In `crates/x402/src/escrow/pda.rs`, change all `pub(crate)` to `pub`:
- `pub fn find_program_address(...)` (line 19)
- `pub fn is_on_ed25519_curve(...)` (line 47)
- `pub fn derive_ata_address(...)` (line 53)
- `pub fn decode_bs58_pubkey(...)` (line 62)
- `pub fn anchor_discriminator(...)` (line 75)
- `pub const ATA_PROGRAM_ID` (line 7)
- `pub const TOKEN_PROGRAM_ID` (line 8)
- `pub const SYSTEM_PROGRAM_ID` (line 9)
- `pub const SYSVAR_RENT_ID` (line 10)

In `crates/x402/src/escrow/mod.rs`, ensure `pda` is publicly re-exported:
```rust
pub mod pda;
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p rustyclawrouter-cli test_x402_escrow_pda_exports_accessible`
Expected: PASS

- [ ] **Step 5: Run full x402 and CLI test suites**

Run: `cargo test -p x402 && cargo test -p rustyclawrouter-cli`
Expected: All tests pass (no behavior change, only visibility change).

- [ ] **Step 6: Commit**

```bash
git add crates/x402/src/escrow/pda.rs crates/x402/src/escrow/mod.rs crates/cli/src/commands/solana_tx.rs
git commit -m "refactor: export x402 escrow PDA helpers for client reuse"
```

---

### Task 2: Add Escrow Deposit Transaction Builder to x402

**Context:** The x402 crate already builds claim transactions in `escrow/claimer.rs` (lines 390-460). The deposit builder follows the same legacy-transaction-building pattern but with different accounts and instruction data. Putting it in x402 lets the CLI (and future Rust clients) reuse it.

**Files:**
- Create: `crates/x402/src/escrow/deposit.rs` — Deposit transaction builder
- Modify: `crates/x402/src/escrow/mod.rs` — Add `pub mod deposit;`

- [ ] **Step 1: Write the failing test**

Create `crates/x402/src/escrow/deposit.rs` with the test first:

```rust
//! Escrow deposit transaction builder.
//!
//! Builds and signs a Solana legacy transaction that calls the escrow program's
//! `deposit` instruction, transferring USDC from the agent's ATA to the
//! vault ATA owned by the escrow PDA.

use super::pda::{
    anchor_discriminator, decode_bs58_pubkey, derive_ata_address, find_program_address,
    ATA_PROGRAM_ID, SYSTEM_PROGRAM_ID, TOKEN_PROGRAM_ID,
};

/// Parameters for building an escrow deposit transaction.
pub struct DepositParams {
    /// Agent's 64-byte keypair (seed || pubkey) as base58.
    pub agent_keypair_b58: String,
    /// Provider (gateway operator) wallet address, base58.
    pub provider_wallet_b58: String,
    /// USDC mint address, base58.
    pub usdc_mint_b58: String,
    /// Escrow program ID, base58.
    pub escrow_program_id_b58: String,
    /// Deposit amount in atomic USDC (6 decimals).
    pub amount: u64,
    /// 32-byte service ID (request correlation ID).
    pub service_id: [u8; 32],
    /// Slot after which the agent can call refund.
    pub expiry_slot: u64,
    /// Recent blockhash (32 bytes).
    pub recent_blockhash: [u8; 32],
}

/// Build and sign an escrow deposit transaction.
///
/// Returns base64-encoded legacy transaction bytes.
///
/// # Errors
///
/// Returns an error string if keypair decoding, address derivation, or
/// signing fails.
pub fn build_deposit_tx(params: &DepositParams) -> Result<String, String> {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keypair_b58() -> String {
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        let verifying_key = signing_key.verifying_key();
        let mut full = [0u8; 64];
        full[..32].copy_from_slice(&seed);
        full[32..].copy_from_slice(verifying_key.as_bytes());
        bs58::encode(&full).into_string()
    }

    fn agent_pubkey_bytes() -> [u8; 32] {
        let seed = [42u8; 32];
        let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
        signing_key.verifying_key().to_bytes()
    }

    const PROVIDER: &str = "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM";
    const USDC_MINT: &str = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
    const ESCROW_PROGRAM: &str = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU";

    #[test]
    fn test_build_deposit_tx_produces_valid_base64() {
        let params = DepositParams {
            agent_keypair_b58: test_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 5000,
            service_id: [1u8; 32],
            expiry_slot: 999_999,
            recent_blockhash: [0u8; 32],
        };
        let result = build_deposit_tx(&params);
        assert!(result.is_ok(), "deposit tx build failed: {:?}", result.err());
        let b64 = result.unwrap();
        // Should be valid base64
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(&b64)
            .expect("output should be valid base64");
        // Minimum size: 1 byte sig count + 64 sig + header + accounts + blockhash + ix
        assert!(decoded.len() > 100, "transaction too short: {} bytes", decoded.len());
    }

    #[test]
    fn test_build_deposit_tx_contains_correct_discriminator() {
        let params = DepositParams {
            agent_keypair_b58: test_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 5000,
            service_id: [1u8; 32],
            expiry_slot: 999_999,
            recent_blockhash: [0u8; 32],
        };
        let b64 = build_deposit_tx(&params).unwrap();
        let tx_bytes = base64::engine::general_purpose::STANDARD.decode(&b64).unwrap();
        // The deposit discriminator should appear in the transaction data
        let disc = anchor_discriminator("deposit");
        assert!(
            tx_bytes.windows(8).any(|w| w == disc),
            "transaction should contain anchor deposit discriminator"
        );
    }

    #[test]
    fn test_build_deposit_tx_contains_agent_pubkey() {
        let params = DepositParams {
            agent_keypair_b58: test_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 5000,
            service_id: [1u8; 32],
            expiry_slot: 999_999,
            recent_blockhash: [0u8; 32],
        };
        let b64 = build_deposit_tx(&params).unwrap();
        let tx_bytes = base64::engine::general_purpose::STANDARD.decode(&b64).unwrap();
        let agent_pk = agent_pubkey_bytes();
        assert!(
            tx_bytes.windows(32).any(|w| w == agent_pk),
            "transaction should contain agent pubkey in account keys"
        );
    }

    #[test]
    fn test_build_deposit_tx_zero_amount_rejected() {
        let params = DepositParams {
            agent_keypair_b58: test_keypair_b58(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 0,
            service_id: [1u8; 32],
            expiry_slot: 999_999,
            recent_blockhash: [0u8; 32],
        };
        let result = build_deposit_tx(&params);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("zero"), "should mention zero amount");
    }

    #[test]
    fn test_build_deposit_tx_invalid_keypair_rejected() {
        let params = DepositParams {
            agent_keypair_b58: "invalid".to_string(),
            provider_wallet_b58: PROVIDER.to_string(),
            usdc_mint_b58: USDC_MINT.to_string(),
            escrow_program_id_b58: ESCROW_PROGRAM.to_string(),
            amount: 5000,
            service_id: [1u8; 32],
            expiry_slot: 999_999,
            recent_blockhash: [0u8; 32],
        };
        let result = build_deposit_tx(&params);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Register the module and run test to verify it fails**

Add `pub mod deposit;` to `crates/x402/src/escrow/mod.rs`.

Run: `cargo test -p x402 deposit`
Expected: FAIL — `todo!()` panics.

- [ ] **Step 3: Implement `build_deposit_tx`**

Replace the `todo!()` with the full implementation. The deposit instruction needs 9 accounts:

```rust
pub fn build_deposit_tx(params: &DepositParams) -> Result<String, String> {
    use base64::Engine;
    use ed25519_dalek::Signer;

    if params.amount == 0 {
        return Err("deposit amount must not be zero".to_string());
    }

    // --- 1. Decode keypair ---
    let key_bytes = bs58::decode(&params.agent_keypair_b58)
        .into_vec()
        .map_err(|e| format!("invalid agent keypair base58: {e}"))?;
    if key_bytes.len() != 64 {
        return Err(format!(
            "agent keypair must be 64 bytes (seed || pubkey), got {}",
            key_bytes.len()
        ));
    }
    let seed: [u8; 32] = key_bytes[..32]
        .try_into()
        .map_err(|_| "failed to slice seed")?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let agent_pubkey = signing_key.verifying_key().to_bytes();

    // Validate derived pubkey matches stored pubkey
    if agent_pubkey != key_bytes[32..] {
        return Err("keypair corrupt: derived pubkey doesn't match stored".to_string());
    }

    // --- 2. Parse addresses ---
    let provider = decode_bs58_pubkey(&params.provider_wallet_b58)
        .map_err(|e| format!("provider: {e}"))?;
    let usdc_mint = decode_bs58_pubkey(&params.usdc_mint_b58)
        .map_err(|e| format!("usdc_mint: {e}"))?;
    let escrow_program_id = decode_bs58_pubkey(&params.escrow_program_id_b58)
        .map_err(|e| format!("escrow_program_id: {e}"))?;
    let token_program = decode_bs58_pubkey(TOKEN_PROGRAM_ID)
        .expect("TOKEN_PROGRAM_ID constant is valid");
    let ata_program = decode_bs58_pubkey(ATA_PROGRAM_ID)
        .expect("ATA_PROGRAM_ID constant is valid");
    let system_program = decode_bs58_pubkey(SYSTEM_PROGRAM_ID)
        .expect("SYSTEM_PROGRAM_ID constant is valid");

    // --- 3. Derive PDAs and ATAs ---
    let (escrow_pda, _bump) = find_program_address(
        &[b"escrow", &agent_pubkey, &params.service_id],
        &escrow_program_id,
    )
    .ok_or("failed to derive escrow PDA")?;

    let agent_ata = derive_ata_address(&agent_pubkey, &usdc_mint)
        .ok_or("failed to derive agent ATA")?;
    let vault_ata = derive_ata_address(&escrow_pda, &usdc_mint)
        .ok_or("failed to derive vault ATA")?;

    // --- 4. Build account keys ---
    // Order matters — matches Deposit accounts struct in escrow program:
    //   0: agent (signer, writable)
    //   1: provider (readonly)
    //   2: mint (readonly)
    //   3: escrow PDA (writable)
    //   4: agent_ata (writable)
    //   5: vault_ata (writable)
    //   6: token_program (readonly)
    //   7: ata_program (readonly)
    //   8: system_program (readonly)
    //   9: escrow_program (program — invoked)
    let account_keys: Vec<[u8; 32]> = vec![
        agent_pubkey,    // 0
        provider,        // 1
        usdc_mint,       // 2
        escrow_pda,      // 3
        agent_ata,       // 4
        vault_ata,       // 5
        token_program,   // 6
        ata_program,     // 7
        system_program,  // 8
        escrow_program_id, // 9
    ];

    // Header: 1 signer (agent), 0 readonly signed, 3 readonly unsigned
    // (mint, token_program, ata_program, system_program = 4 readonly,
    //  but provider is also readonly = 5 readonly unsigned among non-signers)
    // Actually: signer=agent(0). Writable non-signers: escrow(3), agent_ata(4), vault(5).
    // Readonly non-signers: provider(1), mint(2), token_program(6), ata_program(7), system_program(8).
    // Program: escrow_program(9).
    // Solana header: [num_required_signatures, num_readonly_signed, num_readonly_unsigned]
    // num_required_signatures = 1 (agent)
    // num_readonly_signed = 0
    // num_readonly_unsigned = provider(1) + mint(2) + token(6) + ata(7) + system(8) + escrow_program(9) = 6
    let header: [u8; 3] = [1, 0, 6];

    // --- 5. Build instruction data ---
    // Anchor discriminator + amount(u64 LE) + service_id([u8;32]) + expiry_slot(u64 LE)
    let disc = anchor_discriminator("deposit");
    let mut ix_data = Vec::with_capacity(8 + 8 + 32 + 8);
    ix_data.extend_from_slice(&disc);
    ix_data.extend_from_slice(&params.amount.to_le_bytes());
    ix_data.extend_from_slice(&params.service_id);
    ix_data.extend_from_slice(&params.expiry_slot.to_le_bytes());

    // Instruction account indices: [0,1,2,3,4,5,6,7,8] — all accounts except program
    let ix_accounts: Vec<u8> = vec![0, 1, 2, 3, 4, 5, 6, 7, 8];
    let program_id_index: u8 = 9;

    // --- 6. Serialize message ---
    let message_bytes = build_legacy_message(
        &account_keys, header, &params.recent_blockhash,
        &[(program_id_index, ix_accounts, ix_data)],
    );

    // --- 7. Sign ---
    let signature = signing_key.sign(&message_bytes);

    // --- 8. Serialize transaction ---
    let mut tx_bytes = Vec::new();
    tx_bytes.push(1); // compact-u16: 1 signature
    tx_bytes.extend_from_slice(&signature.to_bytes());
    tx_bytes.extend_from_slice(&message_bytes);

    Ok(base64::engine::general_purpose::STANDARD.encode(&tx_bytes))
}

/// Serialize a legacy Solana message (same format as CLI's solana_tx.rs).
fn build_legacy_message(
    account_keys: &[[u8; 32]],
    header: [u8; 3],
    recent_blockhash: &[u8; 32],
    instructions: &[(u8, Vec<u8>, Vec<u8>)],
) -> Vec<u8> {
    let mut msg = Vec::new();
    msg.extend_from_slice(&header);

    // Account keys
    msg.push(account_keys.len() as u8); // compact-u16 for small values (≤127)
    for key in account_keys {
        msg.extend_from_slice(key);
    }

    msg.extend_from_slice(recent_blockhash);

    // Instructions
    msg.push(instructions.len() as u8);
    for (program_id_index, accounts, data) in instructions {
        msg.push(*program_id_index);
        msg.push(accounts.len() as u8);
        msg.extend_from_slice(accounts);
        // Data length as compact-u16
        let data_len = data.len();
        if data_len < 128 {
            msg.push(data_len as u8);
        } else {
            msg.push((data_len & 0x7F) as u8 | 0x80);
            msg.push((data_len >> 7) as u8);
        }
        msg.extend_from_slice(data);
    }

    msg
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p x402 deposit`
Expected: All 5 deposit tests pass.

- [ ] **Step 5: Run full x402 test suite**

Run: `cargo test -p x402`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/x402/src/escrow/deposit.rs crates/x402/src/escrow/mod.rs
git commit -m "feat(x402): add escrow deposit transaction builder"
```

---

### Task 3: CLI Escrow Payment Support

**Context:** The CLI's `chat.rs` currently takes `accepts[0]` (always "exact") and builds a USDC transfer. It needs to: (1) select the escrow scheme when available, (2) generate a service_id from the request body, (3) fetch current slot for expiry calculation, (4) build an escrow deposit transaction, (5) send it as an `EscrowPayload`.

**Files:**
- Modify: `crates/cli/src/commands/chat.rs:77-119` — Scheme selection + escrow branch
- Modify: `crates/cli/src/commands/solana_tx.rs` — Add `build_escrow_deposit` wrapper + `fetch_current_slot`

- [ ] **Step 1: Write tests for scheme selection and escrow flow**

Add tests to `crates/cli/src/commands/chat.rs` test module:

```rust
#[test]
fn test_select_escrow_scheme_prefers_escrow() {
    let accepts = vec![
        x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
        x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: Some("9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU".to_string()),
        },
    ];
    let selected = select_payment_scheme(&accepts);
    assert_eq!(selected.scheme, "escrow");
}

#[test]
fn test_select_escrow_scheme_falls_back_to_exact() {
    let accepts = vec![
        x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
    ];
    let selected = select_payment_scheme(&accepts);
    assert_eq!(selected.scheme, "exact");
}

#[test]
fn test_generate_service_id_length() {
    let body = r#"{"model":"auto","messages":[{"role":"user","content":"test"}]}"#;
    let id = generate_service_id(body.as_bytes());
    assert_eq!(id.len(), 32);
}

#[test]
fn test_generate_service_id_unique_with_nonce() {
    // Same body should produce different IDs due to random nonce
    let body = r#"{"model":"auto","messages":[{"role":"user","content":"test"}]}"#;
    let id1 = generate_service_id(body.as_bytes());
    let id2 = generate_service_id(body.as_bytes());
    assert_ne!(id1, id2, "nonce should make identical requests produce unique service_ids");
}

#[test]
fn test_select_escrow_scheme_skips_escrow_without_program_id() {
    // An accept with scheme="escrow" but no escrow_program_id should be skipped;
    // selection should fall back to the "exact" accept.
    let accepts = vec![
        x402::types::PaymentAccept {
            scheme: "exact".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None,
        },
        x402::types::PaymentAccept {
            scheme: "escrow".to_string(),
            network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp".to_string(),
            amount: "2625".to_string(),
            asset: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v".to_string(),
            pay_to: "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM".to_string(),
            max_timeout_seconds: 300,
            escrow_program_id: None, // missing — should be skipped
        },
    ];
    let selected = select_payment_scheme(&accepts);
    assert_eq!(selected.scheme, "exact");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p rustyclawrouter-cli select_escrow_scheme`
Expected: FAIL — `select_payment_scheme` and `generate_service_id` not found.

- [ ] **Step 3: Implement scheme selection and service_id generation**

In `crates/cli/src/commands/chat.rs`, add:

```rust
use sha2::{Digest, Sha256};

/// Select the preferred payment scheme from the accepts list.
/// Prefers "escrow" (agent gets deposit protection) over "exact".
fn select_payment_scheme(accepts: &[x402::types::PaymentAccept]) -> &x402::types::PaymentAccept {
    accepts
        .iter()
        .find(|a| a.scheme == "escrow" && a.escrow_program_id.is_some())
        .or_else(|| accepts.first())
        .expect("at least one accept required")
}

/// Generate a unique 32-byte service_id by hashing the request body + random nonce.
/// The nonce prevents PDA collisions when identical requests are sent before
/// the first escrow is claimed/refunded.
fn generate_service_id(request_body: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(request_body);
    let nonce: [u8; 8] = rand::random();
    hasher.update(nonce);
    let hash = hasher.finalize();
    let mut id = [0u8; 32];
    id.copy_from_slice(&hash);
    id
}
```

Add `sha2` and `rand` dependencies to `crates/cli/Cargo.toml`:
```toml
sha2 = { workspace = true }
rand = { workspace = true }
```

Check if `sha2` and `rand` are already in the workspace `Cargo.toml`. If not, add them (`sha2` is already a transitive dep via x402; `rand` is used by the nonce generation in `generate_service_id`).

- [ ] **Step 4: Run scheme selection tests**

Run: `cargo test -p rustyclawrouter-cli select_escrow_scheme`
Expected: PASS

Run: `cargo test -p rustyclawrouter-cli generate_service_id`
Expected: PASS

- [ ] **Step 5: Add `fetch_current_slot` to solana_tx.rs**

```rust
/// Fetch the current slot from a Solana JSON-RPC endpoint.
pub async fn fetch_current_slot(rpc_url: &str, client: &reqwest::Client) -> Result<u64> {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "getSlot",
        "params": [{"commitment": "confirmed"}]
    });

    let resp = client.post(rpc_url).json(&body).send().await
        .context("failed to connect to Solana RPC for getSlot")?;
    let json: serde_json::Value = resp.json().await
        .context("failed to parse getSlot response")?;

    json["result"]
        .as_u64()
        .ok_or_else(|| anyhow!("getSlot response missing result field"))
}
```

Add a test:
```rust
#[test]
fn test_fetch_current_slot_bad_rpc() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let rt = tokio::runtime::Runtime::new().unwrap();
    let client = reqwest::Client::new();
    let err = rt.block_on(fetch_current_slot(
        &format!("http://127.0.0.1:{port}"),
        &client,
    )).unwrap_err();
    assert!(err.to_string().contains("connect") || err.to_string().contains("RPC"));
}
```

- [ ] **Step 6: Add `build_escrow_deposit` wrapper to solana_tx.rs**

```rust
/// Build and sign an escrow deposit transaction.
///
/// Wraps `x402::escrow::deposit::build_deposit_tx` with blockhash fetching.
pub async fn build_escrow_deposit(
    payer_keypair_b58: &str,
    provider_wallet: &str,
    escrow_program_id: &str,
    amount: u64,
    service_id: [u8; 32],
    expiry_slot: u64,
    rpc_url: &str,
    client: &reqwest::Client,
) -> Result<String> {
    let recent_blockhash = fetch_blockhash(rpc_url, client).await?;

    x402::escrow::deposit::build_deposit_tx(&x402::escrow::deposit::DepositParams {
        agent_keypair_b58: payer_keypair_b58.to_string(),
        provider_wallet_b58: provider_wallet.to_string(),
        usdc_mint_b58: USDC_MINT.to_string(),
        escrow_program_id_b58: escrow_program_id.to_string(),
        amount,
        service_id,
        expiry_slot,
        recent_blockhash,
    })
    .map_err(|e| anyhow!("escrow deposit tx build failed: {e}"))
}
```

- [ ] **Step 7: Wire escrow flow into chat.rs**

Update the `run` function to branch on scheme. Replace lines 77-119 with:

```rust
    // Select payment scheme (prefer escrow for deposit protection).
    let body_bytes = serde_json::to_vec(&body).unwrap();
    let accepted = select_payment_scheme(&payment_required.accepts).clone();

    // ... (keep RPC URL resolution, lines 84-92) ...

    let (payment_payload, _signed_tx_for_submission) = match accepted.scheme.as_str() {
        "escrow" => {
            let escrow_program_id = accepted
                .escrow_program_id
                .as_deref()
                .context("escrow scheme missing program ID")?;

            let service_id = generate_service_id(&body_bytes);

            // Fetch current slot and compute expiry
            let current_slot = crate::commands::solana_tx::fetch_current_slot(&rpc_url, &client).await
                .context("failed to fetch current slot for escrow expiry")?;
            let timeout_slots = (accepted.max_timeout_seconds as u64 * 1000) / 400; // ~400ms per slot
            let expiry_slot = current_slot + timeout_slots;

            let deposit_tx = crate::commands::solana_tx::build_escrow_deposit(
                private_key_b58,
                &accepted.pay_to,
                escrow_program_id,
                accepted.amount.parse::<u64>().context("invalid amount")?,
                service_id,
                expiry_slot,
                &rpc_url,
                &client,
            )
            .await
            .context("failed to build escrow deposit transaction")?;

            // Derive agent pubkey from keypair
            let key_bytes = bs58::decode(private_key_b58).into_vec().context("keypair decode")?;
            let seed: [u8; 32] = key_bytes[..32].try_into().map_err(|_| anyhow::anyhow!("bad seed"))?;
            let agent_pubkey = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
            let agent_pubkey_b58 = bs58::encode(agent_pubkey.as_bytes()).into_string();

            let payload = PaymentPayload {
                x402_version: x402::types::X402_VERSION,
                resource: Resource {
                    url: "/v1/chat/completions".to_string(),
                    method: "POST".to_string(),
                },
                accepted: accepted.clone(),
                payload: x402::types::PayloadData::Escrow(x402::types::EscrowPayload {
                    deposit_tx: deposit_tx.clone(),
                    service_id: BASE64.encode(service_id),
                    agent_pubkey: agent_pubkey_b58,
                }),
            };
            (payload, deposit_tx)
        }
        _ => {
            // "exact" scheme — direct USDC transfer (existing logic)
            let signed_tx = crate::commands::solana_tx::build_usdc_transfer(
                private_key_b58,
                &accepted.pay_to,
                accepted.amount.parse::<u64>().context("invalid payment amount")?,
                &rpc_url,
                &client,
            )
            .await
            .context("failed to build Solana payment transaction")?;

            let payload = PaymentPayload {
                x402_version: x402::types::X402_VERSION,
                resource: Resource {
                    url: "/v1/chat/completions".to_string(),
                    method: "POST".to_string(),
                },
                accepted: accepted.clone(),
                payload: x402::types::PayloadData::Direct(SolanaPayload {
                    transaction: signed_tx.clone(),
                }),
            };
            (payload, signed_tx)
        }
    };
```

Update the import line (line 3) to include `EscrowPayload`:
```rust
use x402::types::{EscrowPayload, PaymentPayload, PaymentRequired, Resource, SolanaPayload};
```

- [ ] **Step 8: Update the 402 payment test to verify escrow path**

Add a new test `test_chat_402_escrow_payment_flow` that mocks a 402 response with escrow scheme, mocks both `getLatestBlockhash` and `getSlot` RPC calls, and verifies the payment header contains escrow fields:

```rust
#[tokio::test]
async fn test_chat_402_escrow_payment_flow() {
    let _lock = crate::ENV_MUTEX.lock().await;
    let _wallet = setup_wallet();
    let mock = MockServer::start().await;

    // Mock getSlot (must be mounted before getLatestBlockhash — wiremock matches last-mounted first)
    Mock::given(method("POST"))
        .and(path("/"))
        .and(wiremock::matchers::body_string_contains("getSlot"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": 12345
        })))
        .mount(&mock)
        .await;

    // Mock getLatestBlockhash
    Mock::given(method("POST"))
        .and(path("/"))
        .and(wiremock::matchers::body_string_contains("getLatestBlockhash"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": { "value": {
                "blockhash": "11111111111111111111111111111111",
                "lastValidBlockHeight": 9999
            }}
        })))
        .mount(&mock)
        .await;

    let payment_required = serde_json::json!({
        "x402_version": 2,
        "resource": {"url": "/v1/chat/completions", "method": "POST"},
        "accepts": [{
            "scheme": "escrow",
            "network": "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            "amount": "1000",
            "asset": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            "pay_to": "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            "max_timeout_seconds": 300,
            "escrow_program_id": "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
        }],
        "cost_breakdown": {
            "provider_cost": "0.001000",
            "platform_fee": "0.000050",
            "fee_percent": 5,
            "total": "0.001050",
            "currency": "USDC"
        },
        "error": "Payment required"
    });

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(402).set_body_json(serde_json::json!({
            "error": {
                "message": serde_json::to_string(&payment_required).unwrap()
            }
        })))
        .up_to_n_times(1)
        .mount(&mock)
        .await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header_exists("PAYMENT-SIGNATURE"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "choices": [{"message": {"content": "Escrow paid response!"}}]
        })))
        .mount(&mock)
        .await;

    std::env::set_var("SOLANA_RPC_URL", &mock.uri());
    let _env_guard = EnvGuard("SOLANA_RPC_URL");

    let result = run(&mock.uri(), "auto", "What is Solana?", true).await;
    assert!(result.is_ok(), "escrow payment flow should succeed: {:?}", result.err());
}
```

- [ ] **Step 9: Run all CLI tests**

Run: `cargo test -p rustyclawrouter-cli`
Expected: All tests pass.

- [ ] **Step 10: Run lint**

Run: `cargo fmt --all && cargo clippy -p rustyclawrouter-cli --all-targets -- -D warnings`
Expected: Clean.

- [ ] **Step 11: Commit**

```bash
git add crates/cli/src/commands/chat.rs crates/cli/src/commands/solana_tx.rs crates/cli/Cargo.toml
git commit -m "feat(cli): add escrow payment scheme support"
```

**Note:** A `--scheme exact|escrow` CLI flag is a useful follow-up to let callers override the auto-selection, but it is NOT required for the initial implementation. The default (prefer escrow when available) is the correct behavior for deposit protection.

---

### Task 4: Python SDK Escrow Support

**Context:** The Python SDK uses `solders` and `solana-py` for real Solana signing (optional deps). The escrow deposit needs the same libraries plus PDA derivation. The `Pubkey.find_program_address` method in `solders` handles PDA derivation natively.

**Files:**
- Modify: `sdks/python/rustyclawrouter/types.py:60-67` — Add `escrow_program_id` to `PaymentAccept`
- Modify: `sdks/python/rustyclawrouter/x402.py` — Add `build_escrow_deposit`, update `encode_payment_header`
- Modify: `sdks/python/rustyclawrouter/client.py:202-208` — Prefer escrow scheme
- Modify: `sdks/python/tests/test_x402.py` — Escrow tests

- [ ] **Step 1: Write failing test for escrow_program_id on PaymentAccept**

In `sdks/python/tests/test_x402.py` (or create if needed):

```python
def test_payment_accept_with_escrow_program_id():
    accept = PaymentAccept(
        scheme="escrow",
        network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
        amount="5000",
        asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        pay_to="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
        max_timeout_seconds=300,
        escrow_program_id="9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
    )
    assert accept.escrow_program_id == "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
    data = accept.model_dump()
    assert "escrow_program_id" in data
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd sdks/python && python -m pytest tests/test_x402.py::test_payment_accept_with_escrow_program_id -v`
Expected: FAIL — `escrow_program_id` not a valid field.

- [ ] **Step 3: Add `escrow_program_id` to PaymentAccept**

In `sdks/python/rustyclawrouter/types.py`, update `PaymentAccept`:

```python
class PaymentAccept(BaseModel):
    scheme: str
    network: str
    amount: str
    asset: str
    pay_to: str
    max_timeout_seconds: int
    escrow_program_id: Optional[str] = None
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cd sdks/python && python -m pytest tests/test_x402.py::test_payment_accept_with_escrow_program_id -v`
Expected: PASS

- [ ] **Step 5: Write test for escrow payment header encoding**

```python
def test_encode_payment_header_escrow_scheme():
    """Escrow scheme should produce payload with deposit_tx, service_id, agent_pubkey."""
    accept = PaymentAccept(
        scheme="escrow",
        network="solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
        amount="5000",
        asset="EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
        pay_to="9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
        max_timeout_seconds=300,
        escrow_program_id="9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
    )
    # Without private key, should still produce a stub escrow payload
    header = encode_payment_header(accept, "/v1/chat/completions")
    payload = decode_payment_header(header)
    assert "deposit_tx" in payload["payload"]
    assert "service_id" in payload["payload"]
    assert "agent_pubkey" in payload["payload"]
```

- [ ] **Step 6: Implement escrow support in x402.py**

Add `build_escrow_deposit` function and update `build_payment_payload` and `encode_payment_header`:

```python
import hashlib

def build_escrow_deposit(
    pay_to: str,
    amount: int,
    private_key: str,
    escrow_program_id: str,
    service_id: bytes,
    expiry_slot: int,
) -> tuple[str, str]:
    """Build and sign an escrow deposit transaction.

    Returns:
        Tuple of (deposit_tx_b64, agent_pubkey_b58).
    """
    # Import Solana deps (optional)
    try:
        from solana.rpc.api import Client as SolanaClient
        from solders.hash import Hash
        from solders.instruction import AccountMeta, Instruction
        from solders.keypair import Keypair
        from solders.message import MessageV0
        from solders.pubkey import Pubkey
        from solders.system_program import ID as SYSTEM_PROGRAM_ID
        from solders.transaction import VersionedTransaction
        from spl.token.constants import (
            ASSOCIATED_TOKEN_PROGRAM_ID,
            TOKEN_PROGRAM_ID,
        )
    except ImportError:
        raise ImportError(
            "Escrow deposits require: pip install rustyclawrouter[solana]"
        )

    rpc_url = os.environ.get("SOLANA_RPC_URL")
    if not rpc_url:
        raise SigningError("SOLANA_RPC_URL required for escrow deposits")

    kp = Keypair.from_base58_string(private_key)
    usdc_mint = Pubkey.from_string(USDC_MINT)
    provider = Pubkey.from_string(pay_to)
    program_id = Pubkey.from_string(escrow_program_id)

    # Derive escrow PDA
    escrow_pda, _bump = Pubkey.find_program_address(
        [b"escrow", bytes(kp.pubkey()), service_id],
        program_id,
    )

    # Derive ATAs
    agent_ata, _ = Pubkey.find_program_address(
        [bytes(kp.pubkey()), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
        ASSOCIATED_TOKEN_PROGRAM_ID,
    )
    vault_ata, _ = Pubkey.find_program_address(
        [bytes(escrow_pda), bytes(TOKEN_PROGRAM_ID), bytes(usdc_mint)],
        ASSOCIATED_TOKEN_PROGRAM_ID,
    )

    # Build deposit instruction
    discriminator = hashlib.sha256(b"global:deposit").digest()[:8]
    ix_data = (
        discriminator
        + amount.to_bytes(8, "little")
        + service_id
        + expiry_slot.to_bytes(8, "little")
    )

    ix = Instruction(
        program_id=program_id,
        accounts=[
            AccountMeta(kp.pubkey(), is_signer=True, is_writable=True),
            AccountMeta(provider, is_signer=False, is_writable=False),
            AccountMeta(usdc_mint, is_signer=False, is_writable=False),
            AccountMeta(escrow_pda, is_signer=False, is_writable=True),
            AccountMeta(agent_ata, is_signer=False, is_writable=True),
            AccountMeta(vault_ata, is_signer=False, is_writable=True),
            AccountMeta(TOKEN_PROGRAM_ID, is_signer=False, is_writable=False),
            AccountMeta(ASSOCIATED_TOKEN_PROGRAM_ID, is_signer=False, is_writable=False),
            AccountMeta(SYSTEM_PROGRAM_ID, is_signer=False, is_writable=False),
        ],
        data=ix_data,
    )

    # Build and sign transaction
    client = SolanaClient(rpc_url)
    blockhash = client.get_latest_blockhash().value.blockhash
    msg = MessageV0.try_compile(
        payer=kp.pubkey(),
        instructions=[ix],
        address_lookup_table_accounts=[],
        recent_blockhash=blockhash,
    )
    tx = VersionedTransaction(msg, [kp])
    deposit_tx_b64 = base64.b64encode(bytes(tx)).decode()
    agent_pubkey_b58 = str(kp.pubkey())

    return deposit_tx_b64, agent_pubkey_b58


def build_escrow_payment_payload(
    accept: PaymentAccept,
    resource_url: str,
    *,
    deposit_tx_b64: str,
    service_id_b64: str,
    agent_pubkey_b58: str,
    resource_method: str = "POST",
) -> Dict[str, Any]:
    """Build escrow PaymentPayload dict."""
    return {
        "x402_version": X402_VERSION,
        "resource": {"url": resource_url, "method": resource_method},
        "accepted": accept.model_dump(),
        "payload": {
            "deposit_tx": deposit_tx_b64,
            "service_id": service_id_b64,
            "agent_pubkey": agent_pubkey_b58,
        },
    }
```

Update `encode_payment_header` to handle escrow:

```python
def encode_payment_header(
    accept: PaymentAccept,
    resource_url: str,
    resource_method: str = "POST",
    private_key: Optional[str] = None,
    request_body: Optional[bytes] = None,
) -> str:
    if accept.scheme == "escrow" and accept.escrow_program_id:
        # Generate service_id from request body + random nonce (prevents PDA collisions)
        body = request_body or b""
        nonce = os.urandom(8)
        service_id = hashlib.sha256(body + nonce).digest()
        service_id_b64 = base64.b64encode(service_id).decode()

        if private_key:
            try:
                # Fetch current slot for expiry
                from solana.rpc.api import Client as SolanaClient
                rpc_url = os.environ.get("SOLANA_RPC_URL", "")
                client = SolanaClient(rpc_url)
                current_slot = client.get_slot().value
                timeout_slots = int(accept.max_timeout_seconds * 1000 / 400)
                expiry_slot = current_slot + timeout_slots

                amount = int(accept.amount)
                deposit_tx_b64, agent_pubkey_b58 = build_escrow_deposit(
                    pay_to=accept.pay_to,
                    amount=amount,
                    private_key=private_key,
                    escrow_program_id=accept.escrow_program_id,
                    service_id=service_id,
                    expiry_slot=expiry_slot,
                )
            except ImportError:
                raise ImportError(
                    "Escrow deposits require: pip install rustyclawrouter[solana]"
                )
        else:
            deposit_tx_b64 = "STUB_ESCROW_DEPOSIT_TX"
            agent_pubkey_b58 = "STUB_AGENT_PUBKEY"

        payload = build_escrow_payment_payload(
            accept, resource_url,
            deposit_tx_b64=deposit_tx_b64,
            service_id_b64=service_id_b64,
            agent_pubkey_b58=agent_pubkey_b58,
            resource_method=resource_method,
        )
    else:
        # existing "exact" scheme logic (unchanged)
        ...  # keep existing code
```

- [ ] **Step 7: Update client.py scheme selection**

In `_create_payment_header`, prefer escrow and accept `request_body`:

```python
def _create_payment_header(
    self, payment_info: PaymentRequired, resource_url: str,
    request_body: Optional[bytes] = None,
) -> str:
    if not payment_info.accepts:
        raise PaymentError("Gateway returned no accepted payment methods")
    # Prefer escrow scheme for deposit protection
    accept = next(
        (a for a in payment_info.accepts
         if a.scheme == "escrow" and a.escrow_program_id),
        payment_info.accepts[0],
    )
    private_key = self.wallet.private_key if self.wallet.has_key else None
    return encode_payment_header(
        accept, resource_url, private_key=private_key,
        request_body=request_body,
    )
```

**IMPORTANT:** Also update ALL call sites in both sync and async clients to pass `request_body`. In `LLMClient.chat()` (around line 112), change:

```python
# Before:
header = self._create_payment_header(payment_info, url)
# After:
request_body_bytes = json.dumps(request_data).encode()
header = self._create_payment_header(payment_info, url, request_body=request_body_bytes)
```

And in `AsyncLLMClient.chat()` (around line 288), make the same change:

```python
# Before:
header = self._create_payment_header(payment_info, url)
# After:
request_body_bytes = json.dumps(request_data).encode()
header = self._create_payment_header(payment_info, url, request_body=request_body_bytes)
```

For the async client, `encode_payment_header` calls synchronous `SolanaClient.get_slot()` which blocks the event loop. Add a note: if the async client path is used, consider using `solana.rpc.async_api.AsyncClient` instead. For the initial implementation, the sync client is sufficient since most Python SDK users use the sync client.

- [ ] **Step 8: Run Python tests**

Run: `cd sdks/python && python -m pytest tests/ -v`
Expected: All tests pass.

- [ ] **Step 9: Commit**

```bash
git add sdks/python/rustyclawrouter/types.py sdks/python/rustyclawrouter/x402.py \
  sdks/python/rustyclawrouter/client.py sdks/python/tests/
git commit -m "feat(python-sdk): add escrow payment scheme support"
```

---

### Task 5: TypeScript SDK Escrow Support

**Context:** The TypeScript SDK uses `@solana/web3.js` as an optional peer dep for real signing. The escrow deposit needs `PublicKey.findProgramAddressSync` for PDA derivation and `TransactionInstruction` for building the deposit instruction.

**Files:**
- Modify: `sdks/typescript/src/types.ts:47-54` — Add `escrow_program_id` to `PaymentAccept`
- Modify: `sdks/typescript/src/x402.ts` — Add escrow deposit builder + scheme selection
- Modify: `sdks/typescript/tests/x402.test.ts` — Escrow tests

- [ ] **Step 1: Write failing test for escrow types**

In `sdks/typescript/tests/x402.test.ts`:

```typescript
test('escrow payment header contains escrow payload fields', async () => {
  const paymentInfo: PaymentRequired = {
    x402_version: 2,
    accepts: [{
      scheme: 'escrow',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '5000',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: '9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM',
      max_timeout_seconds: 300,
      escrow_program_id: '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU',
    }],
    cost_breakdown: {
      provider_cost: '0.005000', platform_fee: '0.000250',
      total: '0.005250', currency: 'USDC', fee_percent: 5,
    },
    error: 'Payment required',
  };

  const header = await createPaymentHeader(paymentInfo, '/v1/chat/completions');
  const decoded = JSON.parse(Buffer.from(header, 'base64').toString());
  expect(decoded.payload).toHaveProperty('deposit_tx');
  expect(decoded.payload).toHaveProperty('service_id');
  expect(decoded.payload).toHaveProperty('agent_pubkey');
});
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd sdks/typescript && npm test`
Expected: FAIL — `escrow_program_id` not in interface, escrow payload not built.

- [ ] **Step 3: Add `escrow_program_id` to PaymentAccept**

In `sdks/typescript/src/types.ts`:

```typescript
export interface PaymentAccept {
  scheme: string;
  network: string;
  amount: string;
  asset: string;
  pay_to: string;
  max_timeout_seconds: number;
  escrow_program_id?: string;
}
```

- [ ] **Step 4: Update `createPaymentHeader` for escrow**

In `sdks/typescript/src/x402.ts`, add scheme selection and escrow payload building:

```typescript
export async function createPaymentHeader(
  paymentInfo: PaymentRequired,
  resourceUrl: string,
  privateKey?: string,
  requestBody?: string,
): Promise<string> {
  if (!paymentInfo.accepts || paymentInfo.accepts.length === 0) {
    throw new Error('No payment accept options in 402 response');
  }

  // Prefer escrow scheme
  const accept = paymentInfo.accepts.find(
    a => a.scheme === 'escrow' && a.escrow_program_id
  ) || paymentInfo.accepts[0];

  if (accept.scheme === 'escrow' && accept.escrow_program_id) {
    return buildEscrowPaymentHeader(accept, resourceUrl, privateKey, requestBody);
  }

  // Existing "exact" logic (unchanged)
  let transaction = 'STUB_BASE64_TX';
  if (privateKey) {
    const solanaAvailable = isSolanaAvailable();
    if (solanaAvailable) {
      transaction = await buildSolanaTransferChecked(accept.pay_to, accept.amount, privateKey);
    }
  }

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept,
    payload: { transaction },
  };

  const json = JSON.stringify(payload);
  if (typeof btoa === 'function') return btoa(json);
  return Buffer.from(json, 'utf-8').toString('base64');
}

async function buildEscrowPaymentHeader(
  accept: PaymentAccept,
  resourceUrl: string,
  privateKey?: string,
  requestBody?: string,
): Promise<string> {
  const crypto = await import('crypto');
  const bodyBytes = Buffer.from(requestBody || '');
  // Add random nonce to prevent PDA collisions when identical requests are sent
  // before the first escrow is claimed/refunded.
  const nonce = crypto.randomBytes(8);
  const serviceId = crypto.createHash('sha256').update(bodyBytes).update(nonce).digest();
  const serviceIdB64 = serviceId.toString('base64');

  let depositTx = 'STUB_ESCROW_DEPOSIT_TX';
  let agentPubkey = 'STUB_AGENT_PUBKEY';

  if (privateKey && isSolanaAvailable()) {
    const result = await buildEscrowDeposit(
      accept.pay_to,
      accept.amount,
      privateKey,
      accept.escrow_program_id!,
      serviceId,
      accept.max_timeout_seconds,
    );
    depositTx = result.depositTx;
    agentPubkey = result.agentPubkey;
  }

  const payload = {
    x402_version: X402_VERSION,
    resource: { url: resourceUrl, method: 'POST' },
    accepted: accept,
    payload: {
      deposit_tx: depositTx,
      service_id: serviceIdB64,
      agent_pubkey: agentPubkey,
    },
  };

  const json = JSON.stringify(payload);
  if (typeof btoa === 'function') return btoa(json);
  return Buffer.from(json, 'utf-8').toString('base64');
}

async function buildEscrowDeposit(
  payTo: string,
  amountStr: string,
  privateKey: string,
  escrowProgramId: string,
  serviceId: Buffer,
  maxTimeoutSeconds: number,
): Promise<{ depositTx: string; agentPubkey: string }> {
  const {
    Connection, Keypair, PublicKey, TransactionInstruction, TransactionMessage,
    VersionedTransaction, SystemProgram,
  } = await import('@solana/web3.js');
  const { TOKEN_PROGRAM_ID, ASSOCIATED_TOKEN_PROGRAM_ID } = await import('@solana/spl-token');
  const bs58 = await import('bs58');

  const rpcUrl = process.env.SOLANA_RPC_URL;
  if (!rpcUrl) throw new Error('SOLANA_RPC_URL required for escrow deposits');

  const connection = new Connection(rpcUrl, 'confirmed');
  // Use bs58 decoding — same as existing buildSolanaTransferChecked
  const secretKey = bs58.default.decode(privateKey) as Uint8Array;
  const payer = Keypair.fromSecretKey(secretKey);
  const amount = BigInt(amountStr);

  const programId = new PublicKey(escrowProgramId);
  const provider = new PublicKey(payTo);
  const usdcMint = new PublicKey('EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v');

  // Derive escrow PDA
  const [escrowPda] = PublicKey.findProgramAddressSync(
    [Buffer.from('escrow'), payer.publicKey.toBuffer(), serviceId],
    programId,
  );

  // Derive ATAs
  const [agentAta] = PublicKey.findProgramAddressSync(
    [payer.publicKey.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), usdcMint.toBuffer()],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );
  const [vaultAta] = PublicKey.findProgramAddressSync(
    [escrowPda.toBuffer(), TOKEN_PROGRAM_ID.toBuffer(), usdcMint.toBuffer()],
    ASSOCIATED_TOKEN_PROGRAM_ID,
  );

  // Compute expiry slot
  const currentSlot = await connection.getSlot();
  const timeoutSlots = Math.ceil((maxTimeoutSeconds * 1000) / 400);
  const expirySlot = currentSlot + timeoutSlots;

  // Build instruction data
  const crypto = await import('crypto');
  const discriminator = crypto.createHash('sha256').update('global:deposit').digest().subarray(0, 8);
  const data = Buffer.alloc(8 + 8 + 32 + 8);
  discriminator.copy(data, 0);
  data.writeBigUInt64LE(amount, 8);
  serviceId.copy(data, 16);
  data.writeBigUInt64LE(BigInt(expirySlot), 48);

  const ix = new TransactionInstruction({
    programId,
    keys: [
      { pubkey: payer.publicKey, isSigner: true, isWritable: true },
      { pubkey: provider, isSigner: false, isWritable: false },
      { pubkey: usdcMint, isSigner: false, isWritable: false },
      { pubkey: escrowPda, isSigner: false, isWritable: true },
      { pubkey: agentAta, isSigner: false, isWritable: true },
      { pubkey: vaultAta, isSigner: false, isWritable: true },
      { pubkey: TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: ASSOCIATED_TOKEN_PROGRAM_ID, isSigner: false, isWritable: false },
      { pubkey: SystemProgram.programId, isSigner: false, isWritable: false },
    ],
    data,
  });

  // Use VersionedTransaction (V0 message) — same pattern as buildSolanaTransferChecked
  const { blockhash } = await connection.getLatestBlockhash();
  const message = new TransactionMessage({
    payerKey: payer.publicKey,
    recentBlockhash: blockhash,
    instructions: [ix],
  }).compileToV0Message();
  const tx = new VersionedTransaction(message);
  try {
    tx.sign([payer]);

    const serialized = Buffer.from(tx.serialize());
    return {
      depositTx: serialized.toString('base64'),
      agentPubkey: payer.publicKey.toBase58(),
    };
  } finally {
    // Zero the secret key after signing regardless of success or failure
    secretKey.fill(0);
  }
}
```

- [ ] **Step 5: Run TypeScript tests**

Run: `cd sdks/typescript && npm test`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add sdks/typescript/src/types.ts sdks/typescript/src/x402.ts sdks/typescript/tests/
git commit -m "feat(typescript-sdk): add escrow payment scheme support"
```

---

### Task 6: Go SDK Escrow Types

**Context:** The Go SDK only uses stub transactions (no real signing). This task adds the `EscrowProgramID` field to `PaymentAccept` and the escrow payload structure, so it correctly serializes/deserializes 402 responses with escrow schemes. Real signing is a separate future task.

**Files:**
- Modify: `sdks/go/types.go:64-71` — Add `EscrowProgramID` to `PaymentAccept`
- Modify: `sdks/go/x402.go` — Add escrow payload structure, scheme selection
- Modify: `sdks/go/x402_test.go` — Tests

- [ ] **Step 1: Write failing test**

In `sdks/go/x402_test.go`:

```go
func TestCreatePaymentHeaderEscrowScheme(t *testing.T) {
    info := &PaymentRequired{
        X402Version: X402Version,
        Accepts: []PaymentAccept{{
            Scheme:           "escrow",
            Network:          "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
            Amount:           "5000",
            Asset:            "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
            PayTo:            "9WzDXwBbmkg8ZTbNMqUxvQRAyrZzDsGYdLVL9zYtAWWM",
            MaxTimeoutSeconds: 300,
            EscrowProgramID:  "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
        }},
        CostBreakdown: CostBreakdown{
            ProviderCost: "0.005000", PlatformFee: "0.000250",
            Total: "0.005250", Currency: "USDC", FeePercent: 5,
        },
        Error: "Payment required",
    }
    header, err := createPaymentHeader(info, "/v1/chat/completions")
    if err != nil {
        t.Fatalf("unexpected error: %v", err)
    }
    decoded, _ := base64.StdEncoding.DecodeString(header)
    var payload map[string]interface{}
    json.Unmarshal(decoded, &payload)
    p := payload["payload"].(map[string]interface{})
    if _, ok := p["deposit_tx"]; !ok {
        t.Error("escrow payload should contain deposit_tx")
    }
    if _, ok := p["service_id"]; !ok {
        t.Error("escrow payload should contain service_id")
    }
    if _, ok := p["agent_pubkey"]; !ok {
        t.Error("escrow payload should contain agent_pubkey")
    }
    // Verify the accepted scheme is "escrow" in the decoded payload
    accepted, ok := payload["accepted"].(map[string]interface{})
    if !ok {
        t.Fatal("payload should contain accepted field")
    }
    if accepted["scheme"] != "escrow" {
        t.Errorf("expected accepted.scheme to be \"escrow\", got %q", accepted["scheme"])
    }
}

func TestPaymentAcceptEscrowProgramID(t *testing.T) {
    data := `{"scheme":"escrow","network":"solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp","amount":"5000","asset":"EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v","pay_to":"test","max_timeout_seconds":300,"escrow_program_id":"9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"}`
    var accept PaymentAccept
    if err := json.Unmarshal([]byte(data), &accept); err != nil {
        t.Fatalf("unmarshal failed: %v", err)
    }
    if accept.EscrowProgramID != "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU" {
        t.Errorf("expected escrow program ID, got %q", accept.EscrowProgramID)
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cd sdks/go && go test -run TestPaymentAcceptEscrowProgramID -v`
Expected: FAIL — `EscrowProgramID` field not found.

- [ ] **Step 3: Add `EscrowProgramID` to PaymentAccept**

In `sdks/go/types.go`:

```go
type PaymentAccept struct {
    Scheme            string `json:"scheme"`
    Network           string `json:"network"`
    Amount            string `json:"amount"`
    Asset             string `json:"asset"`
    PayTo             string `json:"pay_to"`
    MaxTimeoutSeconds int    `json:"max_timeout_seconds"`
    EscrowProgramID   string `json:"escrow_program_id,omitempty"`
}
```

- [ ] **Step 4: Add escrow payload types and scheme selection to x402.go**

```go
type escrowPayload struct {
    DepositTx   string `json:"deposit_tx"`
    ServiceID   string `json:"service_id"`
    AgentPubkey string `json:"agent_pubkey"`
}

type escrowPaymentPayload struct {
    X402Version int             `json:"x402_version"`
    Resource    paymentResource `json:"resource"`
    Accepted    PaymentAccept   `json:"accepted"`
    Payload     escrowPayload   `json:"payload"`
}

func createPaymentHeader(info *PaymentRequired, resourceURL string) (string, error) {
    if len(info.Accepts) == 0 {
        return "", &PaymentError{Message: "no payment accepts in 402 response"}
    }

    // Prefer escrow scheme
    var accept PaymentAccept
    found := false
    for _, a := range info.Accepts {
        if a.Scheme == "escrow" && a.EscrowProgramID != "" {
            accept = a
            found = true
            break
        }
    }
    if !found {
        accept = info.Accepts[0]
    }

    if accept.Scheme == "escrow" && accept.EscrowProgramID != "" {
        // Stub escrow deposit (real signing is a future feature)
        payload := escrowPaymentPayload{
            X402Version: X402Version,
            Resource:    paymentResource{URL: resourceURL, Method: "POST"},
            Accepted:    accept,
            Payload: escrowPayload{
                DepositTx:   "STUB_ESCROW_DEPOSIT_TX",
                ServiceID:   "STUB_SERVICE_ID",
                AgentPubkey: "STUB_AGENT_PUBKEY",
            },
        }
        jsonBytes, err := json.Marshal(payload)
        if err != nil {
            return "", err
        }
        return base64.StdEncoding.EncodeToString(jsonBytes), nil
    }

    // Existing exact scheme logic
    payload := paymentPayload{
        X402Version: X402Version,
        Resource:    paymentResource{URL: resourceURL, Method: "POST"},
        Accepted:    accept,
        Payload:     solanaPayload{Transaction: "STUB_BASE64_TX"},
    }
    jsonBytes, err := json.Marshal(payload)
    if err != nil {
        return "", err
    }
    return base64.StdEncoding.EncodeToString(jsonBytes), nil
}
```

- [ ] **Step 5: Run Go tests**

Run: `cd sdks/go && go test -v`
Expected: All tests pass.

- [ ] **Step 6: Commit**

```bash
git add sdks/go/types.go sdks/go/x402.go sdks/go/x402_test.go
git commit -m "feat(go-sdk): add escrow payment types and scheme selection"
```

---

## Execution Order

Tasks 1→2 are sequential (Task 2 depends on Task 1's exports).
Tasks 3, 4, 5, 6 are independent of each other (can run in parallel) but all depend on Task 2.

```
Task 1 → Task 2 → ┬─ Task 3 (CLI)
                   ├─ Task 4 (Python)
                   ├─ Task 5 (TypeScript)
                   └─ Task 6 (Go)
```
