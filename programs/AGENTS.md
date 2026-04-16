<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# programs

## Purpose
Standalone Solana Anchor programs. These are **NOT** workspace members — they're built and tested independently to avoid dep-version conflicts with the main Rust workspace (Anchor pins particular versions of `solana-program`, `spl-token`, etc.).

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `escrow/` | Trustless USDC-SPL escrow program — deposit / claim / refund (see `escrow/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- New programs go in their own subdirectory with their own `Cargo.toml` and `Anchor.toml`.
- Never add programs as workspace members; their dep pins conflict with the gateway's crypto stack.
- When a program's public interface changes (account layout, instruction args), also update `crates/x402/src/escrow/` clients and bump a shared constant if appropriate.

### Testing Requirements
Build + test each program independently, e.g.:
```bash
cargo test --manifest-path programs/escrow/Cargo.toml
# Linux OpenSSL workaround if needed:
OPENSSL_NO_PKG_CONFIG=1 OPENSSL_LIB_DIR=/usr/lib/x86_64-linux-gnu OPENSSL_INCLUDE_DIR=/usr/include/openssl cargo test --manifest-path programs/escrow/Cargo.toml
```

### Common Patterns
- Anchor security checklist (see root `AGENTS.md`): typed `Account<'info, T>`, `has_one` checks, PDA seeds with user-specific keys, no `init_if_needed`, checked arithmetic, account closure via `close =`.

## Dependencies

### Internal
- Programs are consumed by `crates/x402/src/escrow/` (client-side).

### External
- `anchor-lang`, `anchor-spl`, `solana-program` (pinned per program).

<!-- MANUAL: -->
