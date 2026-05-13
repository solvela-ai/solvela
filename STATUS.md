# Status

> Live shipping status. See [`CHANGELOG.md`](./CHANGELOG.md) for history, [`SECURITY.md`](./SECURITY.md) for disclosure.

_2026-05-12 (very late) — TypeScript SDK consolidated into the monorepo ([#259](https://github.com/solvela-ai/solvela/pull/259)); tag `sdks/typescript/v0.2.1` pushed; `solvela-ai/solvela-ts` archived with a redirect README. The npm package `@solvela/sdk` is unchanged. Same-PR CI drift coverage now extends to two of four SDKs (Go + TypeScript)._

_2026-05-12 (late) — Go SDK consolidated into the monorepo ([#252](https://github.com/solvela-ai/solvela/pull/252)); tag `sdks/go/v0.1.0` pushed for `proxy.golang.org` discoverability; `solvela-ai/solvela-go` archived with a redirect README. Wire-format drift between gateway and Go SDK is now a same-PR CI failure rather than a post-hoc incident._

_Last refreshed: 2026-05-12 (end of day) — full backlog clear-down. 30 PRs merged, 11 closed without merge, `solvela-protocol@0.2.1` published to crates.io, zero open PRs at session end. **Review-batch closeout** (6 M-tier security fixes from the 2026-05-08 review merged: [#165](https://github.com/solvela-ai/solvela/pull/165) signer-core validation, [#166](https://github.com/solvela-ai/solvela/pull/166) escrow duplicate-ATA guard, [#168](https://github.com/solvela-ai/solvela/pull/168) `tx_raw` truncation in replay logs, [#169](https://github.com/solvela-ai/solvela/pull/169) release.yml shell-injection fix, [#171](https://github.com/solvela-ai/solvela/pull/171) A2A submitted-amount validation, [#172](https://github.com/solvela-ai/solvela/pull/172) per-route replay-LRU namespacing; [#167](https://github.com/solvela-ai/solvela/pull/167) + [#170](https://github.com/solvela-ai/solvela/pull/170) closed as already-superseded by [#192](https://github.com/solvela-ai/solvela/pull/192) / [#198](https://github.com/solvela-ai/solvela/pull/198)). **Deferred CI wave** ([#207](https://github.com/solvela-ai/solvela/pull/207) dashboard CI workflow, [#208](https://github.com/solvela-ai/solvela/pull/208) SDK CI re-runs on `crates/protocol/`, [#209](https://github.com/solvela-ai/solvela/pull/209) cargo-deny, [#210](https://github.com/solvela-ai/solvela/pull/210) informational lcov coverage, [#224](https://github.com/solvela-ai/solvela/pull/224) Go SDK docs rewrite). **Dependabot sweep** (7 patches + 4 verified-safe majors + [#28](https://github.com/solvela-ai/solvela/pull/28) MCP SDK minor; Anchor 0.31→0.32 attempted but closed-as-blocked on upstream `solana-account-info` mismatch — see `Known follow-ups`). **Doc accuracy** ([#225](https://github.com/solvela-ai/solvela/pull/225)–[#227](https://github.com/solvela-ai/solvela/pull/227) earlier sweep + [#229](https://github.com/solvela-ai/solvela/pull/229) protocol wire fix + [#249](https://github.com/solvela-ai/solvela/pull/249)/[#250](https://github.com/solvela-ai/solvela/pull/250) dropped a false deploy-staleness claim that had drifted into both STATUS and v0.1.1 CHANGELOG)._

## Shipped

- **Gateway** — Axum HTTP server with chat completions, image generation, A2A protocol, model registry, escrow endpoints, enterprise org/team/audit/budget endpoints, Prometheus metrics. 5 LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek).
- **Protocol** — `solvela-protocol`, `solvela-x402`, `solvela-router`, `solvela-cli` published to crates.io. v0.1.1 was published with mixed/MIT metadata; v0.2.0 corrects this and aligns with the per-component split (gateway BUSL-1.1, libraries MIT, SDKs MIT). `cargo install solvela-cli` works.
- **Escrow program** — Anchor / USDC-SPL trustless escrow. Deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Last redeployed 2026-05-10 (slot 418832804, tx `58ud2vrh8KsiwST8AsA9qUE7kJpN1NTDvaz8TfzqbG9RRkHM64tUMD974efAigzktVPg5DWXrbE6GWQd6wjFNkxb`, bytecode SHA-256 `5fb8e7548f0653a165803523c322b8d19eab06310462d8621c7250e912bf16c9`) to ship the 2026-05-09 review-pass fixes — vault dust-stuffing drain + minimum expiry buffer + `Transfer` → `TransferChecked` defense-in-depth across all 5 token transfer sites (PRs [#198](https://github.com/solvela-ai/solvela/pull/198) + [#200](https://github.com/solvela-ai/solvela/pull/200)). Prior upgrade 2026-05-08 (slot 418438627, tx `2Rp69yKfyxeeawrFRPZbLma9NytxEXSz8PeWPdNYzWm4e3Fr2xihP3T23PiwDB9xYbnUwN8kmsnrgBKjP2dbBbuE`) closed a claim-vs-refund race at the expiry boundary and a refund deadlock on closed agent ATAs (PR [#160](https://github.com/solvela-ai/solvela/pull/160)). `getProgramAccounts` returned zero at both redeploys; all flaws were latent and unexploited.
- **SDKs** — Python and a wallet-client (Rust) SDK in separate repos: `solvela-python` (v0.1.0), `solvela-client` (v0.2.0). The Go ([#252](https://github.com/solvela-ai/solvela/pull/252)) and TypeScript ([#259](https://github.com/solvela-ai/solvela/pull/259)) SDKs were consolidated into the monorepo on 2026-05-12 and now live at [`sdks/go/`](./sdks/go) (tag `sdks/go/v0.1.0`) and [`sdks/typescript/`](./sdks/typescript) (tag `sdks/typescript/v0.2.1`); the standalone `solvela-ai/solvela-go` and `solvela-ai/solvela-ts` repos are archived with redirect READMEs. Tagged + GitHub Released 2026-04-29 as the security-hardening release; the Go module and `@solvela/sdk` (npm) are live at the new monorepo paths, PyPI/crates.io uploads pending operator credentials.
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
- **Whole-repo security review pass complete (2026-05-09, M-tier closeout 2026-05-12)** — 18 must-ship PRs (#182–#199) + cleanup-tier wave 1 (#200, 8 MEDIUMs) landed across all 6 workspace crates plus the Anchor escrow program; the lower-numbered M-tier batch (#165–#172) was audited 2026-05-12 (2 superseded by the must-ship wave, 6 merged that same day). All HIGH/CRITICAL findings closed in source; escrow bytecode redeployed to mainnet 2026-05-10. See `CHANGELOG.md` and `SECURITY.md` for the per-finding breakdown.

## Repo hardening

- **Branch protection on `solvela-ai/solvela:main`** — 1 PR approval required, 4 required CI checks, branches must be up-to-date, conversation resolution required, force-push and delete blocked.
- **Auto-merge enabled** for dependabot patch/minor batches with required-checks gating.
- **Container hardening** — gateway runtime stage runs as non-root `solvela` user (UID 1001, no home, no login shell) since [#215](https://github.com/solvela-ai/solvela/pull/215) (2026-05-11). Files copied from the build stage use `--chown=solvela:solvela`; verified via CI smoke test that the container responds on `/health` as the non-root user.

## Known follow-ups

- **Registry uploads for SDKs** (PyPI, npm, crates.io) — pending operator credentials.
- **Vercel API token rotation** and **GitHub org 2FA enforcement** — operator-side actions still pending.
- **Anchor 0.31 → 1.0 migration on the escrow program** (#155 / #156) — coordinated bump alongside the broader Solana 1.x → @solana/kit ecosystem migration. Not security-critical now that the deployed bytecode is hardened.
- **Dedicated escrow CI workflow** (#162) — `programs/escrow/` opts out of the workspace and isn't covered by the cross-cutting Rust job. Cargo build-sbf + LiteSVM integration tests should run on every escrow-touching PR.
- **Protocol wire follow-ups deferred from [#229](https://github.com/solvela-ai/solvela/pull/229)** —
  1. **Type split** into `ModelRegistration` (gateway-internal, no Serde) vs `ModelInfo` (wire-only) to make round-trip lossiness structurally impossible.
  2. **Route `GET /v1/models` through `ModelInfo::serialize`** — `crates/gateway/src/routes/models.rs:10-42` currently emits the wire shape via inline `serde_json::json!{}`, creating two parallel definitions and drift risk.
  3. **`compile_fail` doc-test** on `ModelInfo` to catch a future `#[derive(Serialize, Deserialize)]` regression at compile time.
  4. **TS SDK type divergence** — `sdks/typescript/dist/types.d.ts` defines `ModelInfo` as `{ id, object, owned_by }`; verify canonical `solvela-ai/solvela-ts` is in the same state and re-publish if so.
