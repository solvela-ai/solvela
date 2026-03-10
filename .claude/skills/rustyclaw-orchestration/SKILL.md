---
name: rustyclaw-orchestration
description: Project-specific skill router for the RustyClaw monorepo (RustyClawRouter, RustyClawClient). Use when working on Solana payment gateway, x402 protocol, escrow, USDC-SPL, smart routing, or any task in a RustyClaw workspace. Maps files and task types to domain skills (solana-dev, security-review, domain-fintech, etc.) and provides correct cargo verification commands.
---

# Skill: rustyclaw-orchestration

## Purpose

Project-specific skill router for the RustyClaw monorepo. This skill does NOT
define a workflow — superpowers handles that. This skill ensures the right
domain skills get loaded based on which files/areas the task touches.

## When to Activate

Any task in the RustyClaw monorepo (RustyClawRouter or RustyClawClient).

## What This Skill Does

1. Maps the task to domain skills that should be loaded
2. Provides project-specific verification commands
3. Adds session-start check-in after compaction

**This skill defers to superpowers for all workflow decisions** (planning,
TDD, code review, execution, verification, git). It only adds domain context.

---

## Session Start / After Compaction

After compaction or at session start, check in with the user before resuming:
- What were we working on?
- What is the goal for this session?
- Wait for explicit go-ahead.

---

## Skill Routing Table

When a task touches these areas, load the corresponding skills BEFORE doing
any work. Multiple skills can apply — load all that match.

### By File Path

| Files touched | Load these skills |
|---|---|
| `crates/x402/`, `programs/escrow/`, `solana.rs`, `fee_payer.rs`, `nonce_pool.rs`, `facilitator.rs` | `solana-dev` |
| `middleware/x402.rs`, `middleware/rate_limit.rs`, `config.rs` (redaction), `solana.rs` | `security-review` |
| `routes/chat.rs` (USDC amounts), `models.rs` (pricing), `usage.rs` (budgets) | `domain-fintech` |
| `migrations/`, `usage.rs`, `wallet_budgets` | `database-migrations` |
| `usage.rs`, `main.rs` (pool setup) | `postgres-patterns` |
| `routes/`, `middleware/`, `providers/`, `lib.rs` (`build_router`) | `domain-web` |
| `usage.rs`, `cache.rs`, `balance_monitor.rs`, `escrow/claimer.rs`, `main.rs` | `m07-concurrency` |
| `routes/chat.rs`, `routes/services.rs`, `routes/models.rs`, x402 types | `api-design` |
| `Dockerfile`, `docker-compose.yml` | `docker-patterns` |
| `Dockerfile`, `fly.toml` | `deployment-patterns` |
| Any `.rs` file | `rust-router` (non-negotiable — always first for Rust) |

### By Task Type

| Task type | Load these skills |
|---|---|
| New feature or bugfix | `superpowers:test-driven-development` (via superpowers workflow) |
| Schema change / new column | `database-migrations` + `postgres-patterns` |
| Payment/crypto/verification | `solana-dev` + `security-review` + `domain-fintech` |
| API endpoint change | `api-design` + `domain-web` |
| Async/concurrency patterns | `m07-concurrency` |
| Error handling design | `m06-error-handling` |

---

## Project Verification Commands

When superpowers skills need to run verification, use these project-specific
commands (NOT pnpm — this is a Rust workspace):

```bash
# Tests
cargo test                                    # all workspace tests
cargo test -p gateway                         # gateway crate (56 unit + 21 integration)
cargo test -p x402                            # x402 crate (39 tests)
cargo test -p router                          # router crate (13 tests)
cargo test -p rcr-common                      # common crate (10 tests)

# Escrow program (standalone, NOT in workspace)
cargo test --manifest-path programs/escrow/Cargo.toml

# Lint (must pass before committing)
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings

# Quick iteration
cargo check                                   # faster than cargo build
cargo check -p gateway                        # single crate
```

---

## Reminder: What This Skill Does NOT Do

- Does NOT define planning workflow (use `superpowers:writing-plans`)
- Does NOT define TDD workflow (use `superpowers:test-driven-development`)
- Does NOT define code review workflow (use `superpowers:requesting-code-review`)
- Does NOT define execution workflow (use `superpowers:executing-plans` or `superpowers:subagent-driven-development`)
- Does NOT define git workflow (use `superpowers:finishing-a-development-branch`)
- Does NOT define verification workflow (use `superpowers:verification-before-completion`)

It only ensures the right domain skills are loaded so those workflows have
the context they need.
