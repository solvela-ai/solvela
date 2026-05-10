# Status

> Live shipping status. See [`CHANGELOG.md`](./CHANGELOG.md) for history, [`SECURITY.md`](./SECURITY.md) for disclosure.

_Last refreshed: 2026-05-09 — whole-repo security review pass complete; 18 must-ship PRs landed on `main`._

## Shipped

- **Gateway** — Axum HTTP server with chat completions, image generation, A2A protocol, model registry, escrow endpoints, enterprise org/team/audit/budget endpoints, Prometheus metrics. 5 LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek).
- **Protocol** — `solvela-protocol`, `solvela-x402`, `solvela-router`, `solvela-cli` published to crates.io. v0.1.1 was published with mixed/MIT metadata; v0.2.0 corrects this and aligns with the per-component split (gateway BUSL-1.1, libraries MIT, SDKs MIT). `cargo install solvela-cli` works.
- **Escrow program** — Anchor / USDC-SPL trustless escrow. Deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Upgraded 2026-05-08 (slot 418438627, tx `2Rp69yKfyxeeawrFRPZbLma9NytxEXSz8PeWPdNYzWm4e3Fr2xihP3T23PiwDB9xYbnUwN8kmsnrgBKjP2dbBbuE`, bytecode SHA-256 `2293edfce3a0e7b1b0332bc32d9f7a9e58105a2a3f825ee05d8a4f7fbf57bf26`) to patch two latent settlement-path footguns surfaced in pre-launch security review — a claim-vs-refund race at the expiry boundary and a refund deadlock when the agent had closed their USDC ATA. Six secondary hardening items shipped alongside (PR #160). `getProgramAccounts` returned zero at upgrade time; both flaws were latent and unexploited.
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
- **Whole-repo security review pass complete (2026-05-09)** — 18 must-ship PRs (#182–#199) landed across all 6 workspace crates plus the Anchor escrow program. All HIGH/CRITICAL findings closed in source. See `CHANGELOG.md` and `SECURITY.md` for the per-finding breakdown. Cleanup-tier wave 1 ([PR #200](https://github.com/solvela-ai/solvela/pull/200), 8 MEDIUMs) open for review.

## Repo hardening

- **Branch protection on `solvela-ai/solvela:main`** — 1 PR approval required, 4 required CI checks, branches must be up-to-date, conversation resolution required, force-push and delete blocked.
- **Auto-merge enabled** for dependabot patch/minor batches with required-checks gating.
- **Hourly deploy-staleness check** (`.github/workflows/deploy-staleness-check.yml`) opens an issue if production lags `main` HEAD by more than an hour.

## Known follow-ups

- **Registry uploads for SDKs** (PyPI, npm, crates.io) — pending operator credentials.
- **Vercel API token rotation** and **GitHub org 2FA enforcement** — operator-side actions still pending.
- **Escrow program redeploy with #198 + #200 source fixes** — the 2026-05-09 review pass added a vault-dust drain in `claim`, a minimum-expiry-buffer guard in `deposit`, and a `Transfer` → `TransferChecked` migration across all 5 transfer sites. All landed in `main` source; the deployed mainnet bytecode (`9neDH…HLU`) does **not** yet include these. `getProgramAccounts` confirms zero on-chain deposits — both newly-discovered footguns are latent and unexploited (see `SECURITY.md`). Redeploy follows the 2026-05-08 ceremony pattern.
- **Anchor 0.31 → 1.0 migration on the escrow program** (#155 / #156) — coordinated bump alongside the broader Solana 1.x → @solana/kit ecosystem migration. Not security-critical now that the deployed bytecode is hardened.
- **Dedicated escrow CI workflow** (#162) — `programs/escrow/` opts out of the workspace and isn't covered by the cross-cutting Rust job. Cargo build-sbf + LiteSVM integration tests should run on every escrow-touching PR.
