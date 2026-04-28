# Contributing to Solvela

Thanks for your interest. This file covers the basics for getting a change merged.

## Reporting issues

- **Security vulnerabilities** — see [`SECURITY.md`](./SECURITY.md). Email `security@solvela.ai`. Do not open public issues for security reports.
- **Bugs and feature requests** — open an issue with a clear repro (commit SHA, command, expected vs actual). For payment / on-chain bugs, include the transaction signature.

## Development setup

```bash
# 1. Local stack (Postgres + Redis)
docker compose up -d

# 2. Env
cp .env.example .env
# fill in at least one provider API key (OPENAI_API_KEY, ANTHROPIC_API_KEY, ...)

# 3. Run the gateway
RUST_LOG=info cargo run -p gateway     # listens on :8402

# 4. Run the CLI against your local gateway
cargo run -p solvela-cli -- --api-url http://localhost:8402 health
```

See [`CLAUDE.md`](./CLAUDE.md) for the full architecture overview, build matrix, and conventions.

## Submitting a change

1. **Open an issue first** for non-trivial changes so we can align on scope before you write code.
2. **Branch from `main`**, name it `feat/<short>` / `fix/<short>` / `docs/<short>`.
3. **Tests required** for any logic change. Unit tests live next to the code (`#[cfg(test)] mod tests`); integration tests live in `crates/<crate>/tests/`.
4. **Pre-commit local checks** (CI runs the same):
   ```bash
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   cargo test
   ```
5. **Commit messages** follow conventional commits: `<type>(<scope>): <subject>` where type is one of `feat`, `fix`, `refactor`, `docs`, `test`, `chore`, `perf`, `ci`. Keep subject under 70 chars; use the body for the why.
6. **Open the PR** against `main`. The PR description should explain what changed and why, link the issue, and include a short test plan.

## Code conventions

- **Rust 2021 edition.** `cargo fmt` enforces style.
- **Errors**: `thiserror` for library/crate-level error enums, `anyhow` only in binaries / tests. Never `unwrap()` / `expect()` in library code — propagate with `?`.
- **No mutation when avoidable** — prefer returning new values. Prefer `&T` over `&mut T`.
- **No new dependencies without a clear reason** — workspace is intentionally small.
- **Comments are for *why*, not *what***. Code that needs explanation usually needs renaming first.
- **No secrets in code or config files.** Secrets come from env vars only. Custom `Debug` impls must redact (see `crates/gateway/src/config.rs` for the pattern).

## What we won't merge

- Code that adds custodial flows, fiat conversion, or anything triggering MSB/state-licensing requirements.
- New chain support (EVM/Base) without a corresponding `PaymentVerifier` impl behind a feature flag.
- Test mocks that don't match the production gateway response shape (this is what caused the v0.1.0 → v0.1.1 patch).
- Changes that touch the on-chain escrow program without an updated audit reference.

## License

By contributing you agree your contribution is licensed under [MIT](./LICENSE).
