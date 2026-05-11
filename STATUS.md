# Status

> Live shipping status. See [`CHANGELOG.md`](./CHANGELOG.md) for history, [`SECURITY.md`](./SECURITY.md) for disclosure.

_Last refreshed: 2026-05-11 — Python SDK doc surface aligned with `solvela-sdk` 0.2.0 across handbook, dashboard, README, and quickstart (3-PR sweep, tiers A+B); container hardening + doc/runtime parity from earlier same-day work still in effect._

## Shipped

- **Gateway** — Axum HTTP server with chat completions, image generation, A2A protocol, model registry, escrow endpoints, enterprise org/team/audit/budget endpoints, Prometheus metrics. 5 LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek).
- **Protocol** — `solvela-protocol`, `solvela-x402`, `solvela-router`, `solvela-cli` published to crates.io. v0.1.1 was published with mixed/MIT metadata; v0.2.0 corrects this and aligns with the per-component split (gateway BUSL-1.1, libraries MIT, SDKs MIT). `cargo install solvela-cli` works.
- **Escrow program** — Anchor / USDC-SPL trustless escrow. Deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Last redeployed 2026-05-10 (slot 418832804, tx `58ud2vrh8KsiwST8AsA9qUE7kJpN1NTDvaz8TfzqbG9RRkHM64tUMD974efAigzktVPg5DWXrbE6GWQd6wjFNkxb`, bytecode SHA-256 `5fb8e7548f0653a165803523c322b8d19eab06310462d8621c7250e912bf16c9`) to ship the 2026-05-09 review-pass fixes — vault dust-stuffing drain + minimum expiry buffer + `Transfer` → `TransferChecked` defense-in-depth across all 5 token transfer sites (PRs [#198](https://github.com/solvela-ai/solvela/pull/198) + [#200](https://github.com/solvela-ai/solvela/pull/200)). Prior upgrade 2026-05-08 (slot 418438627, tx `2Rp69yKfyxeeawrFRPZbLma9NytxEXSz8PeWPdNYzWm4e3Fr2xihP3T23PiwDB9xYbnUwN8kmsnrgBKjP2dbBbuE`) closed a claim-vs-refund race at the expiry boundary and a refund deadlock on closed agent ATAs (PR [#160](https://github.com/solvela-ai/solvela/pull/160)). `getProgramAccounts` returned zero at both redeploys; all flaws were latent and unexploited.
- **SDKs** — Python, TypeScript, Go, and a wallet-client (Rust) SDK in separate repos: `solvela-python` (v0.1.0), `solvela-ts` (v0.2.0), `solvela-go` (v0.1.0), `solvela-client` (v0.2.0). Tagged + GitHub Released 2026-04-29 as the security-hardening release; Go is live via the module proxy, PyPI/npm/crates.io uploads pending operator credentials.
- **Dashboard + Docs** — Next.js app serving `solvela.ai`, `app.solvela.ai`, `docs.solvela.ai` via subdomain middleware. `www.solvela.ai` 308-redirects to apex.

## Deployed

| Service | URL | Region |
|---|---|---|
| Gateway | `api.solvela.ai` | Fly.io ord |
| Dashboard / Docs | `solvela.ai`, `app.solvela.ai`, `docs.solvela.ai`, `www.solvela.ai` | Vercel |
| Escrow program | mainnet `9neDH…HLU` | Solana |

## Verified

- All-provider end-to-end payment tests pass with real USDC.
- Load tested to ~400 RPS sustained at p99 < 300 ms.
- `cargo test` suite green at HEAD.
- 4 required CI checks gate every merge to `main`: Rust (fmt, clippy, test), Smoke test, Security audit (cargo-audit), Docker build.
- **Whole-repo security review pass complete (2026-05-09)** — 18 must-ship PRs (#182–#199) + cleanup-tier wave 1 (#200, 8 MEDIUMs) landed across all 6 workspace crates plus the Anchor escrow program. All HIGH/CRITICAL findings closed in source; escrow bytecode redeployed to mainnet 2026-05-10. See `CHANGELOG.md` and `SECURITY.md` for the per-finding breakdown.

## Repo hardening

- **Branch protection on `solvela-ai/solvela:main`** — 1 PR approval required, 4 required CI checks, branches must be up-to-date, conversation resolution required, force-push and delete blocked.
- **Auto-merge enabled** for dependabot patch/minor batches with required-checks gating.
- **Hourly deploy-staleness check** (`.github/workflows/deploy-staleness-check.yml`) opens an issue if production lags `main` HEAD by more than an hour.
- **Container hardening** — gateway runtime stage runs as non-root `solvela` user (UID 1001, no home, no login shell) since [#215](https://github.com/solvela-ai/solvela/pull/215) (2026-05-11). Files copied from the build stage use `--chown=solvela:solvela`; verified via CI smoke test that the container responds on `/health` as the non-root user.

## Known follow-ups

- **Registry uploads for SDKs** (PyPI, npm, crates.io) — pending operator credentials.
- **Vercel API token rotation** and **GitHub org 2FA enforcement** — operator-side actions still pending.
- **Anchor 0.31 → 1.0 migration on the escrow program** (#155 / #156) — coordinated bump alongside the broader Solana 1.x → @solana/kit ecosystem migration. Not security-critical now that the deployed bytecode is hardened.
- **Dedicated escrow CI workflow** (#162) — `programs/escrow/` opts out of the workspace and isn't covered by the cross-cutting Rust job. Cargo build-sbf + LiteSVM integration tests should run on every escrow-touching PR.
