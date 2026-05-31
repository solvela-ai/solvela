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
- Test mocks that don't match the production gateway response shape. This has bitten us before — keep mocks honest, or use the real `tower::ServiceExt::oneshot` integration-test pattern in `crates/gateway/tests/`.
- Changes that touch the on-chain escrow program without an updated audit reference.

## License and Developer Certificate of Origin

Solvela uses a **per-component license split**. By contributing, you agree that your contribution is licensed under the license of the component you modify, as declared in that component's `Cargo.toml` / `package.json` / file headers. See the [Licensing table in README.md](./README.md#licensing) for the full breakdown:

- Gateway (`crates/gateway`) — **BUSL-1.1** (transitions to MIT on the Change Date)
- Protocol / x402 / router / cli crates and the escrow program — **Apache-2.0**
- SDKs and the dashboard — **Apache-2.0**

### Sign-off (DCO) is required

Every commit must include a `Signed-off-by` line. This is the [Developer Certificate of Origin](https://developercertificate.org/) — a lightweight statement that you have the right to submit your contribution under the applicable license. There is no separate CLA to sign.

The repo ships with tracked git hooks in `.githooks/` that handle DCO automatically. Activate them once per clone:

```bash
git config core.hooksPath .githooks
```

That enables `prepare-commit-msg` (auto-injects a `Signed-off-by:` trailer matching your `user.name` / `user.email` on every commit, idempotent) and `commit-msg` (enforces the trailer matches the author identity, mirroring the GitHub-side DCO check). The trailer is generated from the `user.email` in your local git config — make sure it's the address you want on the public commit log.

If you'd rather sign manually, `git commit -s` works too. The legacy `git config format.signoff true` / `commit.signoff true` flags do **not** reliably cover all git invocation paths (notably HEREDOC `-m` from scripts) — the `.githooks/` setup is the source of truth.

A `Signed-off-by: Your Name <your.email@example.com>` line at the end of the commit message is what CI checks.

### Relicensing notice

Solvela may, at its sole discretion, relicense any component under another **OSI-approved permissive open-source license** (for example, moving between the MIT and Apache-2.0 licenses, or to a successor permissive license). By signing off on your commits via the DCO, you authorize the project to do this without further consent. The project will not relicense your contribution under a proprietary or copyleft license, or otherwise materially more restrictive terms, without your agreement.

> The protocol/x402/router/cli crates, the escrow program, the SDKs, and the dashboard moved from MIT to Apache-2.0 under this provision (2026-05). The gateway remains BUSL-1.1.
