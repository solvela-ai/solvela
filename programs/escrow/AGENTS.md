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
- Run the full Anchor security checklist (root `AGENTS.md`): typed accounts, has_one validations, checked arithmetic, account closure via `close =`, duplicate-account guard. **`init_if_needed` exception**: permitted only for Associated Token Accounts (`associated_token::*`) because ATA addresses are deterministic from `(mint, owner)` — re-init of an existing ATA is a safe no-op (Anchor checks `is_initialized` first) and an attacker can't substitute a different account. The program uses this exception in `deposit.rs` (vault), `claim.rs` (provider + agent token accounts), and `refund.rs` (agent token account). Do not extend it to arbitrary accounts; the root AGENTS.md checklist's "Reinitialization-attack vector must be ruled out explicitly" applies.
- Account sizing uses `#[derive(InitSpace)]` + `8 + Escrow::INIT_SPACE` (8-byte discriminator prefix).
- Never mutate this program's state layout without a versioned migration plan — existing on-chain accounts will become unreadable.

### Testing Requirements
```bash
# Default: unit tests only (tests/unit.rs) — runs from a fresh checkout, no .so needed.
cargo test --manifest-path programs/escrow/Cargo.toml

# Full suite — LiteSVM integration tests in tests/integration.rs are gated
# behind the `sbf` feature because they include_bytes! the compiled program
# from target/deploy/solvela_escrow.so. Run `anchor build` first.
anchor build
cargo test --manifest-path programs/escrow/Cargo.toml --features sbf

# If Linux pkg-config picks up wrong OpenSSL (prepend to either command):
OPENSSL_NO_PKG_CONFIG=1 OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test --manifest-path programs/escrow/Cargo.toml
```
Prefer LiteSVM for fast deterministic tests; use Surfpool/mainnet-fork only when you need real-account behaviour.

### Build Fallback
If `anchor build` exits silently without producing `target/deploy/solvela_escrow.so` (a known foot-gun on some toolchain combinations — Anchor 0.31.1 vs newer cargo), use the underlying SBF builder directly:

```bash
cd programs/escrow && cargo build-sbf
```

This produces the same `.so` artifact and runs the same SBF compilation pipeline that `anchor build` invokes — just without Anchor's outer wrapping. Verified to produce a valid program in this repo's tooling configuration.

### Deployment Checklist

**Cluster policy: per-cluster program IDs (issue #120 path A).** `declare_id!` in `src/lib.rs` is feature-gated: the DEFAULT build declares the deployed mainnet ID `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` (byte-identical to the previous mainnet-only artifact); building with `--features devnet` declares the devnet rehearsal ID `GyJRAC46bDoBQJNzTnbupWj8T62GipFKnizHExLXPwvo`. The devnet program keypair is operator-held and never committed. `Anchor.toml` carries `[programs.mainnet]` and `[programs.devnet]`; `[programs.localnet]` remains intentionally absent — local integration tests run via LiteSVM (`tests/integration.rs`), not `anchor deploy`, so a localnet deploy fails fast with "program not found".

The `mainnet` feature flag selects which USDC mint the deployed program accepts at compile time. **Mainnet deploys MUST be built with `--features mainnet`** — without it, the program targets the devnet USDC mint (`4zMM…`) and would silently reject all mainnet USDC deposits with `mint mismatch`. This feature flag is independent of the program-ID selection above and exists only because the same source compiles for both LiteSVM (default, devnet mint) and mainnet deploys. `mainnet` and `devnet` are mutually exclusive (`compile_error!` in `lib.rs`): a devnet build pairs the devnet ID with the default devnet mint.

```bash
# Mainnet deploy — mainnet ID (default) + EPjFW…USDC mint
cargo build-sbf -- --features mainnet

# Devnet rehearsal deploy — devnet ID + 4zMM…devnet USDC mint
cargo build-sbf -- --features devnet

# LiteSVM / test build (no on-chain deploy) — mainnet ID + 4zMM…USDC mint
cargo build-sbf
```

`tests/unit.rs::test_program_id_matches_active_feature` pins the ID per feature configuration; run `cargo test` both with and without `--features devnet` after touching either `declare_id!`.

Before invoking `solana program deploy`, verify the resulting `.so` is the right variant by inspecting the constants:

```bash
cargo expand --features mainnet | grep 'USDC_MINT.*=' | head -1
# Expect: pub const USDC_MINT: Pubkey = pubkey!("EPjFW…");
```

There is no on-chain self-check for which mint the deployed bytecode references — the program would happily accept the configured-at-compile-time mint. The discipline lives in this checklist.

### Lint Notes (informational)
`cargo clippy --all-targets -- -D warnings` against this crate emits ~14 `unexpected cfg condition value` warnings on newer Rust toolchains. These originate inside Anchor 0.31.1's macro-generated code (`#[program]`, `#[derive(Accounts)]`) and don't reflect any issue with this program's source. Workspace clippy (`cargo clippy --workspace`) doesn't surface them because escrow is not a workspace member. Either tolerate the noise locally, or add `RUSTFLAGS="--cap-lints=warn"` to your dev shell.

### Common Patterns
- `#[error_code]` enum with `#[msg("…")]` on every variant.
- CPI with PDA signer using `CpiContext::new_with_signer` (see root AGENTS.md for the canonical snippet).
- Require-guards: `require!(cond, EscrowError::Foo);`.
- Token transfers use `TransferChecked` (mint + decimals validated at the SPL token program level), not bare `Transfer`.

## Dependencies

### Internal
- Client-side counterpart in `crates/x402/src/escrow/`.

### External
- `anchor-lang`, `anchor-spl` (for `token::transfer_checked` CPIs), `solana-program`.

<!-- MANUAL: -->
