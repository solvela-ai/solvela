<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# tests

## Purpose
Tests for the escrow Anchor program. Mix of unit tests (pure logic) and integration tests (LiteSVM-driven program execution).

## Key Files
| File | Description |
|------|-------------|
| `helpers.rs` | Shared test fixtures — LiteSVM bootstrap, keypair generators, mint setup, instruction builders |
| `unit.rs` | Pure-logic unit tests — arithmetic guards, PDA derivation, state layout sanity checks |
| `integration.rs` | End-to-end LiteSVM tests — deposit/claim/refund flow, expiry behaviour, unauthorised-caller rejection |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Prefer LiteSVM over Mollusk/Surfpool — it's fast, deterministic, and needs no external runtime.
- `svm.warp_to_slot(n)` to test expiry-based flows.
- `svm.airdrop(&pubkey, lamports)` before constructing transactions.
- Every new instruction needs: happy path + each `require!` failure path + unauthorised caller.

### Testing Requirements
```bash
cargo test --manifest-path programs/escrow/Cargo.toml
cargo test --manifest-path programs/escrow/Cargo.toml integration
```

### Common Patterns
- Build transactions with `Transaction::new_signed_with_payer`.
- Assert `svm.send_transaction(tx)` both `.is_ok()` and `.is_err()` explicitly — don't just `.unwrap()`.
- Keep helper builders deterministic (seeded keypairs) so tests are reproducible.

## Dependencies

### Internal
- The escrow program binary compiled from `../src/`.

### External
- `litesvm`, `solana-sdk`, `anchor-lang`, `spl-token`.

<!-- MANUAL: -->
