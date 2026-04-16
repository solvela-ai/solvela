<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
Escrow program source. `lib.rs` is the Anchor entry point (declares the program ID and routes instructions); `state.rs` defines on-chain accounts; `errors.rs` defines the error enum; `instructions/` contains per-instruction handlers.

## Key Files
| File | Description |
|------|-------------|
| `lib.rs` | `#[program]` module — entry points for deposit / claim / refund |
| `state.rs` | `Escrow` account layout (agent, provider, mint, amount, service_id, expiry_slot, bump) with `#[derive(InitSpace)]` |
| `errors.rs` | `EscrowError` enum — `NotExpired`, `ClaimExceedsDeposit`, etc. |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `instructions/` | Per-instruction accounts + handlers: deposit, claim, refund (see `instructions/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- Adding a new instruction: (1) new file under `instructions/`, (2) register in `instructions/mod.rs`, (3) wire the `#[program]` entry point in `lib.rs`.
- Any change to `Escrow` struct layout is a breaking on-chain change — plan a migration.
- Keep `lib.rs` thin — only program-ID declaration and instruction dispatch.

### Testing Requirements
```bash
cargo test --manifest-path programs/escrow/Cargo.toml
```

### Common Patterns
- Anchor account macros: `Account<'info, T>`, `Signer<'info>`, `SystemAccount<'info>`, `Program<'info, Token>`.
- PDA seeds include caller-specific keys — no shared globals.
- Use `#[account]` with `has_one = …` for authority checks instead of manual validation.

## Dependencies

### Internal
_(none — standalone)_

### External
- `anchor-lang`, `anchor-spl`.

<!-- MANUAL: -->
