<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# escrow

## Purpose
Trustless USDC-SPL escrow. Agents deposit USDC into a PDA keyed by `(agent, service_id)`; the service provider claims up to the deposited amount after doing the work; any remainder refunds to the agent after the expiry slot. Program is built and tested standalone (not a workspace member).

## Key Files
| File | Description |
|------|-------------|
| `Anchor.toml` | Anchor program config — cluster, program keypair path |
| `Cargo.toml` | Standalone manifest; **not** a workspace member |
| `Cargo.lock` | Pinned deps for reproducible builds |
| `mainnet-program-keypair.json` | Program ID keypair for mainnet deployment |
| `buffer-signer.json` | Buffer signer keypair used during program upgrades |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | Anchor program source — `lib.rs`, state, errors, instructions (see `src/AGENTS.md`) |
| `tests/` | Unit + integration tests, including LiteSVM-based simulations (see `tests/AGENTS.md`) |
| `target/` | Anchor/cargo build output (not checked in) |

## For AI Agents

### Working In This Directory
- PDA seeds: `[b"escrow", agent.key().as_ref(), &service_id]` — keep in sync with `crates/x402/src/escrow/pda.rs`.
- Run the full Anchor security checklist (root `AGENTS.md`): typed accounts, has_one validations, no `init_if_needed`, checked arithmetic, account closure via `close =`, duplicate-account guard.
- Account sizing uses `#[derive(InitSpace)]` + `8 + Escrow::INIT_SPACE` (8-byte discriminator prefix).
- Never mutate this program's state layout without a versioned migration plan — existing on-chain accounts will become unreadable.

### Testing Requirements
```bash
cargo test --manifest-path programs/escrow/Cargo.toml
# If Linux pkg-config picks up wrong OpenSSL:
OPENSSL_NO_PKG_CONFIG=1 OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test --manifest-path programs/escrow/Cargo.toml
```
Prefer LiteSVM for fast deterministic tests; use Surfpool/mainnet-fork only when you need real-account behaviour.

### Common Patterns
- `#[error_code]` enum with `#[msg("…")]` on every variant.
- CPI with PDA signer using `CpiContext::new_with_signer` (see root AGENTS.md for the canonical snippet).
- Require-guards: `require!(cond, EscrowError::Foo);`.

## Dependencies

### Internal
- Client-side counterpart in `crates/x402/src/escrow/`.

### External
- `anchor-lang`, `anchor-spl` (for `token::transfer` CPIs), `solana-program`.

<!-- MANUAL: -->
