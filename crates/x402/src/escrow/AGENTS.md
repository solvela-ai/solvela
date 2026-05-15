<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# escrow

## Purpose
Client-side integration with the trustless USDC-SPL escrow Anchor program in `programs/escrow/`. Handles PDA derivation, deposit/claim/refund transaction construction, claim-queue persistence, and asynchronous claim processing.

## Key Files
| File | Description |
|------|-------------|
| `mod.rs` | Module root; re-exports client entry points |
| `pda.rs` | Escrow PDA derivation — seeds: `[b"escrow", agent.key().as_ref(), &service_id]` |
| `deposit.rs` | Builds and submits escrow `deposit` instructions |
| `claim.rs` → see `claimer.rs` | (historic split — see `claimer.rs` / `claim_processor.rs` / `claim_queue.rs`) |
| `claimer.rs` | High-level `claim` submitter — used by the gateway after a provider call succeeds |
| `claim_queue.rs` | Persistent queue of pending claims (PostgreSQL via the `postgres` feature) |
| `claim_processor.rs` | Background worker that drains the claim queue with retry + backoff (`next_retry_at` from migration 004) |
| `refund.rs` | Builds and submits escrow `refund` instructions (post-expiry) |
| `verifier.rs` | Verifies an escrow-scheme x402 payment: checks PDA owner, deposit amount, expiry slot |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- PDA seeds are **this project's convention**: `[b"escrow", agent.key().as_ref(), &service_id]`. Do not change without updating the on-chain program in sync.
- Claim queue operations are gated on the `postgres` feature — keep them `#[cfg(feature = "postgres")]` so the crate stays usable without sqlx.
- Use checked arithmetic everywhere (`checked_add`, `checked_sub`) — escrow deals with funds.
- Fire-and-forget claim submission: the gateway writes to `claim_queue` synchronously, but a background task drains it. Never block a user request on claim settlement.

#### `service_id` MUST mix a per-request CSPRNG nonce (security invariant)

`crates/x402/src/escrow/` accepts `service_id: [u8; 32]` as an input — derivation is the client's responsibility. **Clients MUST mix a per-request CSPRNG nonce into the `service_id` hash**, not derive it as a pure function of `request_body` alone. Without the nonce, two identical request bodies produce the same `service_id` → the same escrow PDA → the same vault ATA, all of which are computable off-chain from `(agent_pubkey, service_id_derivation_rule, USDC_MINT)` *before* the deposit broadcasts. That enables:

1. **Front-running ATA creation** — an attacker pre-creates the vault ATA. Post-#115 this no longer breaks claim, but is still a confusion vector.
2. **Confidentiality leak** — an on-chain observer who knows the derivation rule can correlate vault addresses to specific prompts/models. Service traffic patterns become decodable from the public ledger.
3. **Pre-deposit grief** — pre-creating the vault ATA with dust skews telemetry counters.

The on-chain program treats `service_id` as opaque bytes, so this is a purely off-chain discipline. The nonce becomes part of the off-chain receipt and is persisted alongside the deposit (e.g. in `claim_queue.service_id`) so the gateway can re-derive PDAs at claim time.

Current implementations:

- **Rust CLI**: `crates/cli/src/commands/chat.rs::generate_service_id` — SHA-256 of `(request_body, 8-byte CSPRNG nonce via getrandom)`. Regression test: `tests::test_generate_service_id_unique_with_nonce`.
- **TS signer-core**: `sdks/signer-core/src/sign.ts::buildEscrowPaymentHeader` — SHA-256 of `(bodyBytes, 8-byte CSPRNG via node:crypto.randomBytes)`. Regression test: `'service_id differs across calls (random component)'` in `sdks/signer-core/tests/sign.test.ts`.

If you add a new client (Go SDK, in-tree gateway test fixture, etc.), mirror the same `SHA-256(body || nonce)` shape and add an `identical_body_distinct_service_ids` regression test. Filed under issue #118.

### Testing Requirements
```bash
cargo test -p x402 escrow
```
LiteSVM is the preferred local simulator for deeper tests (see root AGENTS.md for example).

### Common Patterns
- Typed errors via `thiserror` — `EscrowError::NotExpired`, `EscrowError::ClaimExceedsDeposit`, etc.
- Instruction builders return `solana_sdk::instruction::Instruction` (not raw bytes).
- Retries use exponential backoff with a capped ceiling.

## Dependencies

### Internal
- `crate::solana`, `crate::solana_rpc`, `crate::solana_types`, `crate::traits::PaymentVerifier`.

### External
- `sha2`, `base64`, `bs58`, `serde`, `thiserror`, `tracing`, `sqlx` (optional, `postgres` feature).

<!-- MANUAL: -->
