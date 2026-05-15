# journey — onboarding-flow simulator

> Mechanical checks catch *exact* drift. Agentic checks catch *interpretive*
> drift. This checks *experiential* drift — the bugs that only show up when
> a real user follows the docs verbatim.

The original bug that motivated all this audit work was a user running
`solvela chat` after following the Rust CLI quickstart — and getting a
confusing error because `SOLANA_RPC_URL` wasn't in the docs. No regex
could catch that. An LLM page review *could*, but only probabilistically.
This check catches it deterministically: it actually runs the commands.

## Scope (intentionally narrow for PR-1)

One scenario: the Rust CLI quickstart (`dashboard/content/docs/sdks/rust.mdx`).

Each scenario is a list of `Step` objects mirroring the commands in the doc.
The runner applies the doc's `export FOO=BAR` lines to a controlled env,
runs each step, and compares the actual outcome against the spec.

**Out of scope for PR-1:**
- Docker isolation (CI runner is acceptable; the steps don't pollute global
  state much beyond `~/.solvela/wallet.json` which is per-tempdir).
- Real USDC payment (we can't fund a wallet in CI, so paid steps assert
  specific dead-end errors rather than success).
- Other SDK quickstarts (TS / Python / Go). Each adds a new
  `scenarios/<name>.py` file in a follow-up PR.

## Running it

```bash
# Build the CLI first (the journey shells out to it).
cargo build --release -p solvela-cli

# Run the Rust CLI scenario locally.
python scripts/audit-docs/audit.py --journey rust_cli

# List available scenarios.
python scripts/audit-docs/audit.py --journey help

# JSON output for CI artifacts.
python scripts/audit-docs/audit.py --journey rust_cli --format json
```

The runner uses a **controlled environment** — only the scenario's declared
env vars plus a minimal `PATH` / `HOME` are present. The user's personal env
vars don't leak in. This is intentional: we want to reproduce what a brand-new
user following the docs would see, not what the audit author's own setup
happens to do.

## How it catches the original bug

The Rust CLI scenario's `chat_smoke_test` step runs `solvela chat "test"`
with the env vars **the doc tells the user to export**. If `SOLANA_RPC_URL`
isn't among them, `solvela chat` will fail with `"SOLANA_RPC_URL required"`
— and the step's expected error pattern is the *wallet/payment* failure,
not the env-var failure. Mismatch → finding.

If the docs are correct, the step gets past env-var validation, fails at
wallet/payment as expected, and the scenario passes.

## CI integration

`docs-audit-journey.yml` runs on:
- `workflow_dispatch` (manual trigger, useful before a release)
- Weekly cron (Sundays 06:00 UTC) — catches doc drift that happens between
  active PRs

Not on every PR. The Rust build + journey execution takes 3–5 minutes; not
worth that latency for every change. If a PR specifically touches the
quickstart, the developer can trigger the workflow manually.

## Cost

Free. No LLM calls. The only cost is CI minutes (~5 min per run on
`ubuntu-latest`).
