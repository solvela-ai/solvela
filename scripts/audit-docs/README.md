# audit-docs

Mechanical drift detector for Solvela's documentation. Catches the class of bug where the docs say one thing and the code does another — wrong CLI flags, undocumented env vars, stale numeric claims, broken links.

Runs in CI on every PR that touches docs, the CLI surface, the model registry, or the SDK version. Fails the build on `severity=error`; emits warnings for borderline cases.

## What it checks

| Check | What it verifies |
|---|---|
| `cli_examples` | Every `solvela …` line in a fenced bash block uses a real subcommand and only real flags (cross-referenced against the binary's own `--help`). |
| `env_vars` | Every env var read by `crates/` source code has a doc mention, and every `export FOO=` in the docs corresponds to a real read. Legacy `RCR_*` aliases are matched against their `SOLVELA_*` canonicals. |
| `subcommand_coverage` | Every `Commands::X` variant in `crates/cli/src/main.rs` has at least one doc example. |
| `numeric_claims` | Curated claims (fee %, model count, default URLs, SDK/CLI versions) match their source-of-truth files. |
| `links` | Internal markdown links resolve; anchor links match real headings; external links HEAD-check (warning only — external flakes don't block CI). |

## Running locally

```bash
# Build the CLI first (the example check shells out to --help)
cargo build --release -p solvela-cli

# Run all checks, pretty output
python scripts/audit-docs/audit.py

# Run a single check
python scripts/audit-docs/audit.py --check cli_examples

# JSON output (for CI artifacts / agentic phase 2)
python scripts/audit-docs/audit.py --format json > findings.json

# Treat warnings as errors
python scripts/audit-docs/audit.py --strict
```

Exit codes: `0` = clean, `1` = at least one error finding, `2` = invocation error (missing dep, broken fixture).

## Adding a new check

1. Drop a `checks/your_check.py` file exposing a `run(ctx: AuditContext) -> list[Finding]` function.
2. Register it in `audit.py`'s `CHECKS` dict.
3. Add unit tests in `tests/test_your_check.py` with fixture docs under `tests/fixtures/`.

## Why this exists

A previous docs sweep missed two real bugs that broke a user's first paid `solvela chat`: an undocumented `SOLANA_RPC_URL` requirement and a `--stream` flag that wasn't real. Surface-level review of each file in isolation can't catch this — the checks here verify each doc claim against an executable source of truth.

Out of scope for now (deferred to Phase 2 / follow-up PRs):

- Code-block compilation (rust/typescript/python). Needs per-language harnesses.
- Onboarding journey simulation (run the quickstart in a clean container).
- Agentic semantic review (coverage gaps, claim verification, voice consistency).
- PR-comment posting. CI fail/pass is the signal for now.
