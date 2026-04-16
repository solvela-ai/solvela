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
