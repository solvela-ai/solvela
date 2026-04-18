# MCP Plugin for Claude Code / Cursor / OpenClaw — Implementation Plan

**Date:** 2026-04-18
**Author:** Planner (Solvela)
**Revision:** 1.4 (2026-04-18 — rev 1.3 review amendments applied (`2026-04-18-mcp-plugin-plan-review-r1.3.md`): escrow tool implementation task added; session persistence pulled into Phase 1; npm scope + publish automation made explicit; Phase 4 → Phase 2 dependency made explicit; 402 fixture ownership documented; P2 nits fixed.)
**Target package:** `@solvela/mcp-server` (existing, v0.1.0) + `mcp install` subcommand folded into the existing Rust `solvela` CLI (§4.4)
**Sibling artifact:** `@solvela/openclaw-provider` (approved per §4.2 — ships sequenced after MCP server)
**Strategy source:** `docs/strategy/2026-04-17-competitive-analysis.md` §5 item 7
**Status:** Revision 1.4 — all review amendments applied. Ready for execution kickoff.

---

## 1. Overview

### Goal

Ship a production-grade MCP (Model Context Protocol) server for Solvela that one-line installs into Claude Code, Cursor, Claude Desktop, and OpenClaw, with **real** on-chain USDC payment signing (not the current stub). Conditional on §4.2: additionally ship an OpenClaw Provider Plugin that registers Solvela as a first-class LLM provider with per-call x402 signing via `wrapStreamFn`.

This is distribution-channel work. The competitive analysis (§5 item 7) flags it as one of three ship-now moves: meet agents where they already live.

**Package name:** `@solvela/mcp-server`. A prior README stub referenced `@solvela/mcp` — that name is NOT used; 0.1.0 was published as `@solvela/mcp-server` (verified in `sdks/mcp/package.json:2`). The README discrepancy is fixed in Phase 1 T1-J.

### Why now

- **Strategic window is 60–90 days** per the competitive analysis before category compression closes.
- **Sibling plan (`2026-04-16-vercel-ai-sdk-provider-plan.md`) is already authoring a signer adapter** — the MCP plan must coordinate, not duplicate.
- **The existing `@solvela/mcp-server` package is a stub** (`client.ts:242` returns `'STUB_BASE64_TX'`). It will embarrass Solvela if a user installs it and every payment fails. Either we harden it or we unpublish it.

### Non-goals (V1)

- Full multi-wallet support (hardware, Phantom deeplink, MPC). V1 ships local keypair signer only; adapter interface leaves room to add others.
- EVM / Base payment paths (CLAUDE.md rule 6).
- Browser-side MCP (MCP over stdio only for Node; HTTP transport for remote use cases).
- OpenClaw Skills package (markdown prompt-injection layer) — separate plan.
- Tool coverage beyond the existing 5 (`chat`, `smart_chat`, `wallet_status`, `list_models`, `spending`). A sixth `deposit_escrow` tool is proposed in §4.5 as an open decision.
- Changes to the gateway (`crates/gateway`). Consumer of existing contract only.
- Direct Solana RPC health probing from the MCP server in `wallet_status`. Deferred to V1.1; V1.0 trusts the gateway's `/health` response (see §5 S9, §8).

---

## 2. Research snapshot

### 2.1 Existing Solvela code (read, not assumed)

| File | Status | Implication |
|---|---|---|
| `sdks/mcp/src/client.ts:242` | **Stub** — `buildPaymentHeader` emits `'STUB_BASE64_TX'`. | Phase 1 must replace this. |
| `sdks/mcp/src/index.ts` | MCP server skeleton works; 5 tools wired. | Keep. Rewire client. |
| `sdks/mcp/src/index.ts:35-40` | `RCR_SESSION_BUDGET`, `RCR_API_URL`, `RCR_TIMEOUT_MS` are **primary** reads (not fallbacks). | T1-G renames all to `SOLVELA_*` in Phase 1 before T2-B ships. |
| `sdks/mcp/src/client.ts:111` | `RCR_API_URL` is a fallback after `SOLVELA_API_URL` — partial migration already. | T1-G completes it. |
| `sdks/mcp/src/client.ts:146` | Non-atomic budget read-check-then-write — two parallel calls can both pass the check. | T1-H (moved from T5-A) adds `async-mutex` in Phase 1. |
| `sdks/typescript/src/x402.ts` | **Production signer** — `createPaymentHeader`, `buildEscrowDeposit`, `buildSolanaTransferChecked`. Prefers escrow scheme, falls back to transfer-checked. Zeroes secret bytes after signing. Uses `@solana/web3.js` as optional peer dep. | Reuse. Do not reimplement. Dependency: `@solvela/sdk`. |
| `sdks/typescript/package.json:3-4` | `"main": "dist/index.js"` only — **no `exports` map**. Subpath import `@solvela/sdk/x402` will throw `ERR_PACKAGE_PATH_NOT_EXPORTED`. | T1-A.5 resolves this before T1-B begins. |
| `sdks/mcp/package.json:6` | `"type": "module"` (ESM). | ESM/CJS interop is the B1 blocker; T1-A.5 is the concrete spike. |
| `sdks/typescript/src/wallet.ts` | Thin `Wallet` class — derives address from base58 key. No signing. | Keep as-is. Signer lives in `x402.ts`. |
| `sdks/mcp/README.md:10` | Uses `@solvela/mcp` (incorrect; package is `@solvela/mcp-server`). | T1-J fixes in Phase 1. |
| `sdks/mcp/tests/server.test.ts:50-63` | Tests assert `RCR_API_URL` behavior. | Update in T1-G to use `SOLVELA_*`. |
| `crates/gateway/tests/fixtures/402-envelope.json` | Static fixture exists but no test consumes it. | T1-I adds a real contract test in Phase 1. |
| `crates/gateway/src/routes/chat/mod.rs:146` | `dev_bypass_payment` silently skips payment verification. | T1-D startup check warns when this mode is detected. |
| `docs/superpowers/plans/2026-04-16-vercel-ai-sdk-provider-plan.md` §3.4 | Defines `SolvelaWalletAdapter` interface. `createLocalWalletAdapter` is the dev/test reference impl. | **Coordination requirement.** See §9. |

**Phase 1 sizing:** wiring `createPaymentHeader` from `@solvela/sdk` into the MCP client is roughly 50–80 LOC of delete + import + pass-through. Days, not weeks. T1-A.5 must resolve ESM/CJS interop before that work lands.

### 2.2 Host configuration surfaces

Verified against live docs (see §10 for raw excerpts):

| Host | Config file | Transport | Env vars | One-click install |
|---|---|---|---|---|
| **Claude Code** | `.mcp.json` (project) or user-scope via `claude mcp add` | stdio (default), HTTP/SSE | `env` block on server entry, or `-e` flag on `claude mcp add` | **Anthropic MCP Registry** at `api.anthropic.com/mcp-registry/v0/servers` — listing feeds the UI picker in Claude Code and Claude Desktop. |
| **Cursor** | `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global) | stdio (`type: "stdio"`) or HTTP/SSE (`url`) | `env` block + `envFile` (stdio only) + `${env:VAR}` interpolation in strings | **cursor.directory** and Cursor Marketplace; deeplink `cursor://` scheme installs with "Add to Cursor" button. |
| **OpenClaw (MCP)** | `~/.openclaw/openclaw.json` → `mcp.servers` | stdio (`command`/`args`), SSE (`url`), **streamable-http** (`transport: "streamable-http"`) | `env` block on stdio; `headers` on HTTP/SSE | `openclaw mcp set <name> '<json>'` CLI. No marketplace documented; discoverability via the project's llms.txt index. |
| **OpenClaw (Provider Plugin)** | `openclaw.plugin.json` in plugin root + `registerProvider(...)` at runtime | N/A — direct gateway access | `providerAuthEnvVars` in manifest, `wrapStreamFn` hook injects `PAYMENT-SIGNATURE` before each call | Install via npm; `openclaw` loads installed plugins through the SDK. See §4.2. |

All four surfaces accept environment variables for the wallet key. This is the correct secret-propagation path (MCP host never puts secrets in tool arguments, which are model-controlled).

The Rust `solvela` CLI handles installer generation for all platforms. Distributed via: (a) npm meta-package `@solvela/cli` with platform-specific `optionalDependencies` (primary path — `npm i -g @solvela/cli`), and (b) direct GitHub Releases downloads for users who prefer not to use npm. Both paths serve the same single Rust binary; no drift class.

### 2.3 The OpenClaw Provider Plugin opportunity

OpenClaw's `registerProvider` API includes a `wrapStreamFn` hook that fires **before every inference call**, allowing per-request header injection. Combined with `api: "openai-completions"` and a custom `baseUrl`, this allows Solvela to appear inside OpenClaw as a **first-class model provider** — not a tool the agent has to be prompted to call.

For Claude Code and Cursor, no equivalent exists: MCP is the only extension surface. Solvela can only appear as a tool (`chat`, `smart_chat`).

Implication: a Provider Plugin gives OpenClaw users a dramatically better UX (Solvela models appear in the model picker, agent pays transparently). This is an explicit scope increase and is surfaced as **§4.2**, not quietly folded into the plan.

### 2.4 Competitor MCP landscape

| Player | MCP offering | Signal |
|---|---|---|
| BlockRun | None shipped as of 2026-04-17. GitHub has experiments only. | Gap — first-mover advantage in LLM-gateway MCP for Solvela. |
| Skyfire | No public MCP; SaaS dashboard only. | Gap. |
| Nevermined | MCP monetization story exists (per competitive analysis); not a Solana x402 offering. | Parallel path. |
| OpenRouter | No first-party MCP; community wrappers exist. | Gap — but if they ship one, Solvela's USDC-gated tooling remains differentiated. |
| Bankr x402 Cloud | No MCP documented. | Gap. |
| GateRouter | No MCP documented. | Gap. |

**Finding:** No direct competitor has shipped a production MCP server that handles payment signing. Solvela can be **the first Solana-native x402 MCP server** if Phase 1 ships cleanly.

---

## 3. What I need from you (consolidated)

All items must be settled before execution begins. Numbered items map to sections below.

### Decisions (§4) — ALL APPROVED 2026-04-18

- [x] **§4.1 — Default signing model** — APPROVED Option C: **auto** (escrow-preferred when gateway advertises escrow accept, direct TransferChecked fallback). Reuses `x402.ts:45-48` preference logic. Controlled via `SOLVELA_SIGNING_MODE=auto|escrow|direct|off`.
- [x] **§4.2 — Scope for "OpenClaw"** — APPROVED Option B: **MCP + Provider Plugin**. Two artifacts, sequenced — MCP server ships Phase 2, Provider Plugin ships Phase 3.
- [x] **§4.3 — Signer coordination** — APPROVED Option A: **depend on `@solvela/sdk`** for v1.0. Migrate to `@solvela/signer-core` if/when the sibling AI SDK plan triggers its §3.8 extraction. Follow-up ticket to file in HANDOFF after both V1s ship.
- [x] **§4.4 — Installer CLI** — APPROVED Hybrid C: **fold `mcp install` subcommand into the existing Rust `solvela` CLI** (`crates/cli`) AND distribute via npm `optionalDependencies` as `@solvela/cli` (turborepo/biome pattern). Single Rust source of truth; `npm i -g @solvela/cli` UX on all platforms (T4-G.1–T4-G.6, user decision 2026-04-18 r1.3).
- [x] **§4.5 — `deposit_escrow` MCP tool** — APPROVED Option A: **ship the tool**, gated behind `SOLVELA_ESCROW_MODE=enabled` and capped by `SOLVELA_MAX_ESCROW_DEPOSIT` env var (default $5.00 per call) AND `SOLVELA_MAX_ESCROW_SESSION` (default $20.00 cumulative per session; see §4.5 details).
- [x] **§4.6 — Distribution targets for V1** — APPROVED Option D: **npm + Anthropic MCP Registry + cursor.directory + OpenClaw docs PR + solvela.ai/blog launch post + HN Show + X thread + Solana Foundation grant update.** Align Phase 4 with launch window.

### Access / credentials

- [ ] **npm publish rights for `@solvela/mcp-server`** (already 0.1.0 — user confirms they own the scope).
- [ ] **npm org scope `@solvela`** — confirm user owns the npm org (not just individual package names). Required for T4-G.4 platform packages: `@solvela/cli`, `@solvela/cli-linux-x64`, `@solvela/cli-linux-arm64`, `@solvela/cli-win32-x64`, `@solvela/cli-darwin-x64`, `@solvela/cli-darwin-arm64`.
- [ ] **Anthropic MCP Registry submission** — confirm submission process and whether manual review gates listing. Document-specialist subagent to research before Phase 4.
- [ ] **cursor.directory listing submission** — confirm PR process to https://github.com/pontusab/directories or whatever upstream hosts it. Document-specialist subagent to research before Phase 4.
- [ ] **OpenClaw docs PR / blog submission** — confirm upstream repo for `https://docs.openclaw.ai` if Solvela wants first-party docs inclusion.
- [ ] **Funded devnet wallet** for CI smoke tests. ≥0.10 devnet USDC, SOL for rent.
- [ ] **GitHub PAT for Phase 4 community submissions** — `repo` scope for forks/PRs.

### Risk gates

- [ ] Security review of the replacement `client.ts` before npm publish (secret redaction, error paths, key-zeroization preserved). `pr-review-toolkit:silent-failure-hunter` + `oh-my-claudecode:security-reviewer`.
- [ ] If a user installs the plugin with only `SOLANA_WALLET_ADDRESS` and no `SOLANA_WALLET_KEY`, the server **must** refuse to make a paid call — not fall through to a stub that the gateway will reject at verification. Explicit startup-time check.

---

## 4. Decisions (all approved 2026-04-18)

> All six decisions in this section were approved by the user on 2026-04-18 with the planner-recommended options. The options analysis is retained below for audit trail and for future revisions that may revisit the choice. Each subsection carries an **APPROVED** header with the locked option.

### 4.1 Default signing model — APPROVED: Option C (auto)

**Options:**
- **A. Hot-key only** — `SOLANA_WALLET_KEY` env var (base58) unlocks signing. Simple. Hot wallet on disk.
- **B. Escrow-session only** — user runs `solvela escrow deposit` once, MCP server reuses the session PDA. No hot key in env for paid calls. Matches product moat.
- **C. Both** — read `SOLVELA_SIGNING_MODE` env (`escrow` | `direct` | `auto`). Auto prefers escrow when the gateway's 402 response advertises an escrow accept (the TS SDK already does this: see `x402.ts:45-48`). Fallback to direct transfer when the gateway doesn't advertise escrow for that route.

**Recommendation: C, with default `auto`.** The TS SDK already implements this preference logic. The MCP server should inherit it. Users who want to force one or the other set the env var. Matches "escrow-first by default" positioning.

**Impact of changing after ship:** low — env var is additive.

### 4.2 Scope for "OpenClaw" — APPROVED: Option B (MCP + Provider Plugin, sequenced)

**Options:**
- **A. MCP only** — one `@solvela/mcp-server` package, registered via `openclaw mcp set`. Ships in the same artifact as Claude Code + Cursor.
- **B. MCP + Provider Plugin** — add a second package `@solvela/openclaw-provider` that registers Solvela as a first-class LLM provider with `wrapStreamFn` per-call signing. Two packages, two distribution flows, but dramatically better OpenClaw UX.
- **C. Provider Plugin only** — no MCP for OpenClaw. Risk: OpenClaw users who prefer tool-calling UX lose access.

**Recommendation: B.** The Provider Plugin is the only path that makes Solvela appear in OpenClaw's model picker — meeting agents where they live is the whole point of the plan. Ship MCP first (Phase 2), Provider Plugin second (Phase 3). Don't bundle — separate artifacts allow independent iteration.

**Impact of changing after ship:** medium — adds a new npm package with its own release cycle.

### 4.3 Signer coordination with sibling plan — APPROVED: Option A (depend on `@solvela/sdk` for v1.0)

**Context:** `2026-04-16-vercel-ai-sdk-provider-plan.md` §3.4 defines `SolvelaWalletAdapter` as an interface; `createLocalWalletAdapter` is the dev/test reference. §3.8 contemplates extracting a `@solvela/signer-core` package if ESM/CJS interop forces it.

**Options:**
- **A. MCP server depends on `@solvela/sdk`** directly (imports `createPaymentHeader` from `@solvela/sdk/x402`). Matches current code. Zero new packages.
- **B. MCP server depends on the future `@solvela/signer-core`** once the sibling plan extracts it. Shared abstraction across all signer consumers.
- **C. MCP server defines its own adapter interface** independently. Risk: two parallel impls drift.

**Recommendation: A for V1.0, migrate to B if/when the sibling plan extracts signer-core.** Document the migration as a follow-up in `HANDOFF.md`. The MCP server's dependency manifest should pin `@solvela/sdk ^0.x` so a future major bump triggers deliberate upgrade.

**Impact of changing after ship:** low — dependency swap.

### 4.4 Installer CLI — APPROVED: Hybrid C (cross-compile via `cargo-dist` + npm `optionalDependencies` distribution)

**Options:**
- **A. New `@solvela/mcp-install` npm package** — ships `solvela-mcp-install --host=claude-code` etc.
- **B. Fold into the existing Rust `solvela` CLI** — `solvela mcp install --host=cursor` writes the right config file. Rust binary ships for all platforms via `cargo-dist`. (r1.2 approved option, superseded by Hybrid C in r1.3.)
- **C. Hybrid** — fold `mcp install` into the Rust `solvela` CLI (same as B), AND distribute the compiled binary through npm `optionalDependencies` as `@solvela/cli` with platform-specific sub-packages, following the turborepo/biome pattern. Users get `npm i -g @solvela/cli` UX backed by a single Rust codebase — no drift class.

**Recommendation: B** — preserved for audit trail. See r1.3 update below.

**Impact of changing after ship:** medium — adds a Rust subcommand and tests; npm distribution adds ~4–8h of one-time scaffold.

**r1.3 update (2026-04-18):** after the R8 research addendum (`2026-04-18-mcp-plugin-plan-review-r8-addendum.md`) surfaced that the dependency tree is already Windows-cross-compile-safe AND that the turborepo/biome npm-optionalDependencies pattern solves the `npm i -g` UX without introducing drift, the approved option shifts from pure Option B to **Hybrid C**. Single Rust source of truth (r1.2's win) plus native-feeling npm install (r1.1's win, minus the parity-risk wrapper). Effort increases from 6–8h to 12–16h; ongoing per-release effort is unchanged at ~30 min.

### 4.5 `deposit_escrow` MCP tool — APPROVED: Option A (ship, gated + capped)

**Context:** If the user chooses escrow mode (§4.1), they need a way to top up the session. A "run this command in your terminal" flow from the MCP server is awkward.

**Options:**
- **A. Add tool** — `deposit_escrow { amount_usdc: string }` signs + submits a deposit from the configured key, returns the PDA + expiry.
- **B. Don't add tool** — user runs `solvela escrow deposit` in their terminal. Explicit, outside the chat flow.

**Recommendation: A, gated behind `SOLVELA_ESCROW_MODE=enabled`.** If the agent hits a "budget exceeded" or "escrow expired" during `chat`, it can call `deposit_escrow` itself. Dangerous if the model is adversarial — hence the gate.

**Two-layer cap:**
- `SOLVELA_MAX_ESCROW_DEPOSIT` (default $5.00) — per-call maximum. A single `deposit_escrow` invocation cannot request more than this.
- `SOLVELA_MAX_ESCROW_SESSION` (default $20.00) — cumulative cap across the entire MCP server session. The `deposit_escrow` handler (T2-G) tracks cumulative deposits against `~/.solvela/mcp-session.json` (written by T1-K in Phase 1 — not T5-B as in rev 1.3; rev 1.4 promoted persistence forward to close the restart-bypass window) and refuses when the session total would exceed this cap. This closes the adversarial-loop attack: a model cannot drain the wallet $5 at a time regardless of how many times it calls the tool, and survives server restarts via the persisted counter.

**Impact of changing after ship:** low — additive tool.

### 4.6 Distribution targets for V1 — APPROVED: Option D (full launch slate)

**Options:**
- **A. npm only** — publish `@solvela/mcp-server@1.0.0` and `@solvela/openclaw-provider@1.0.0`.
- **B. npm + Anthropic MCP Registry** — submit server manifest to `api.anthropic.com/mcp-registry`.
- **C. npm + Anthropic Registry + cursor.directory** — PR to cursor.directory / Cursor Marketplace.
- **D. All of C + OpenClaw docs PR + HN launch post + solvela.ai/blog post.**

**Recommendation: D.** Publishing without discovery is 80% of the work for 20% of the traction. Align Phase 4 with launch.

**Impact of changing after ship:** high — distribution decisions shape the go-to-market narrative.

---

## 5. Success criteria (concrete, testable, user-facing)

| # | Criterion | How verified |
|---|---|---|
| S1 | `npm install -g @solvela/mcp-server@1.0.0` produces a working install under Node ≥18 ESM. | CI matrix, Node 18/20/22. |
| S2 | `solvela mcp install --host=claude-code` writes a valid `.mcp.json` entry that Claude Code picks up; `chat` tool appears in the tool list. | Integration test: spawn Claude Code in a temp dir, use `claude mcp list` to verify. |
| S3 | `solvela mcp install --host=cursor` writes `.cursor/mcp.json` with the correct stdio config; Cursor reload loads the server. | Manual QA + unit test asserting exact JSON output. |
| S4 | `solvela mcp install --host=openclaw` runs `openclaw mcp set solvela '<json>'` and returns success. | Mocked `openclaw` CLI in test; integration test with real binary on CI. |
| S5 | A `chat` call against a mocked 402-then-200 gateway signs and retries with a **real** base64-encoded VersionedTransaction (not `STUB_BASE64_TX`) as verified by `decodePaymentHeader`. Passes the concurrency race test (S10 promoted to Phase 1). | Integration test IT-1. |
| S6 | A `chat` call against a mocked 402 escrow response triggers `buildEscrowDeposit` path; returned header contains a `deposit_tx` not starting with `STUB_`. | Integration test IT-2. |
| S7 | Starting the server with `SOLANA_WALLET_ADDRESS` set but `SOLANA_WALLET_KEY` unset causes a clear startup error naming the missing var. Server does not fall through to stub. | Unit test. |
| S8 | Private key bytes never appear in MCP tool responses, `McpError.message`, or stderr logs (grep for first 8 chars of a known test key). | `pr-review-toolkit:silent-failure-hunter` + sentinel-fixture test. |
| S9 | `wallet_status` reports gateway health and the gateway's advertised `solana_rpc` URL. Direct RPC health probing from the MCP server is deferred to V1.1; V1.0 trusts the gateway's `/health` response. | Integration test. |
| S10 | `spending` budget enforcement is concurrency-safe (T1-H, Phase 1): two simultaneous `chat` calls against a $0.10 budget with a $0.08 cost each result in exactly one success and one `SolvelaBudgetExceededError` — never two successes, never two failures mid-flight. | Unit test (see S5; race test passes in Phase 1). |
| S11 | `deposit_escrow` tool signs and submits a real escrow deposit; returned PDA exists on chain. | Integration test with devnet wallet. |
| S11.5 | `deposit_escrow` refuses when session cumulative deposits would exceed `SOLVELA_MAX_ESCROW_SESSION`. Verified by integration test that calls `deposit_escrow` with 5 × $4 amounts against a $20 session cap — 4 succeed, 5th returns `SolvelaBudgetExceededError`. | Integration test. |
| S12 | OpenClaw Provider Plugin — `openclaw models list` shows Solvela models after install; `openclaw chat --model solvela/claude-sonnet-4` signs per-call via `wrapStreamFn`. | Integration test. |
| S13 | Hallway test: external tester follows README cold, successfully completes one `chat` call via Claude Code. First sticky-point reported for Phase 6 iteration. | Phase 6 gate. |
| S14 | Anthropic MCP Registry entry approved; Solvela appears in Claude Code's MCP picker. | Phase 4 gate. |
| S15 | cursor.directory listing approved with "Add to Cursor" button. | Phase 4 gate. |

---

## 6. Phased implementation

### Phase 0 — Cross-plan coordination

> **This plan is gated on one of the following from the sibling Vercel AI SDK provider plan (`2026-04-16-vercel-ai-sdk-provider-plan.md`):**
>
> - (a) `@solvela/sdk` exports-map PR lands (enables clean ESM imports for both plans), OR
> - (b) Sibling plan's §3.8 Option B triggers, extracting `@solvela/signer-core` as an ESM-native standalone package, OR
> - (c) This plan's T1-A.5 spike confirms the current CJS `@solvela/sdk` is ESM-consumable as-is (bare import from entry point works without subpath, and internal `require()` calls do not throw).
>
> **Phase 1 begins when any of (a)/(b)/(c) is verified.** If all three fail, both plans pause for user decision on signer strategy. Do not start T1-B before Phase 0 resolves.
>
> **Bidirectional note:** the sibling plan's out-of-scope section excludes edits to `@solvela/mcp-server`. The sibling plan's coordination section should mark `@solvela/mcp-server` as a consumer whose signing dependency must remain stable across any `@solvela/sdk` restructuring. This is a cross-plan courtesy note; logged here and in §9. The sibling plan's file is NOT edited in this revision — that edit belongs to the sibling plan's next revision.

### Phase 1 — Replace stub signing with `@solvela/sdk`

**Size: 2–3 days if T1-A.5 spike passes (includes T1-K session persistence, promoted from Phase 5 per rev 1.4); 4–6 days if spike fails and requires dual-publish or signer-core extraction.**

**Task order is significant.** T1-A.5 must complete before T1-B. T1-G must complete before Phase 2 (T2-B) begins.

Tasks:
- **T1-A.** Add `@solvela/sdk` as a dependency in `sdks/mcp/package.json`. Peer-depend `@solana/web3.js`, `@solana/spl-token`, `bs58`.
- **T1-A.5.** ESM/CJS interop spike + `@solvela/sdk` exports-map PR. *(Must complete before T1-B.)*
  - (a) Run a minimal ESM script under Node 18/20/22 that imports `{ createPaymentHeader }` from `@solvela/sdk` (not a subpath) and calls it against a mocked 402 payload. If the synchronous `require()` inside `x402.ts` throws `ReferenceError: require is not defined` in ESM, escalate to Option B.
  - (b) If the bare import works, add an `exports` map to `sdks/typescript/package.json` mapping `"."` to `dist/index.js` and `"./x402"` to `dist/x402.js` for future use. Otherwise, hold off on the exports map and document the ESM-only barrel-import pattern.
  - (c) If the spike fails (Option B triggered): either (1) dual-publish `@solvela/sdk` as ESM+CJS via `tsup` or similar, OR (2) extract `@solvela/signer-core` as a separate ESM-native package — coordinate with the sibling Vercel AI SDK provider plan's §3.8 decision. Do NOT vendor/inline the signer into the MCP server.
  - (d) Document PASS/FAIL outcome in `.omc/plans/open-questions.md` under "mcp-plugin Phase 1 T1-A.5 spike results." If FAIL, pause for user to choose among the sibling plan's §3.8 options (planner recommends Option B: extract `@solvela/signer-core`).
- **T1-B.** Delete `buildPaymentHeader` (the stub). Import `createPaymentHeader` from `@solvela/sdk/x402` (or bare `@solvela/sdk` entry if T1-A.5 requires it).
- **T1-C.** Route the `GatewayClient.chat` 402 path through `createPaymentHeader(paymentInfo, url, process.env.SOLANA_WALLET_KEY, requestBody)`.
- **T1-D.** Add startup checks:
  - If `SOLVELA_SIGNING_MODE` is not `off` and no `SOLANA_WALLET_KEY` is set, emit a clear error and exit (do not silently stub). Exception: `SOLVELA_SIGNING_MODE=off` explicitly disables payment for demo / CI use.
  - Call `GET /health` on the configured gateway URL at startup. Log the gateway's `dev_bypass_payment` flag if present. If true and `SOLVELA_ALLOW_DEV_BYPASS=1` is NOT set, emit a WARN-level log line to stderr (not an error — user may intentionally point at a dev gateway). Do not refuse to start; this is warn-only.
- **T1-E.** Add `SOLANA_RPC_URL` startup check when signing is enabled. Fail-fast if missing.
- **T1-F.** Update `tests/server.test.ts` to assert the header payload decodes to a payload with a real (base64-length ≥ 200 bytes) `transaction` or `deposit_tx`, not `STUB_*`.
- **T1-G.** Rename legacy `RCR_*` env vars to `SOLVELA_*` throughout `src/`, `tests/`, and `README.md`. Accept `RCR_*` silently (no warning log) for one patch release window; drop at 1.0.0. Rationale: package is at 0.1.0 with minimal external users; full-stack deprecation machinery adds maintenance cost for zero upside. Tests at `sdks/mcp/tests/server.test.ts:50-63` update to use `SOLVELA_*` directly. **T1-G must complete before T2-B begins.**
- **T1-H.** Concurrency-safe budget enforcement: wrap `GatewayClient.sessionSpent` in an `async-mutex` so two parallel 402 flows cannot both claim the last budget dollar. (~10 LOC; ~0.5 days, absorbed into Phase 1 sizing.) *(Moved from T5-A.)*
- **T1-I.** Create `sdks/mcp/tests/contract.test.ts` that loads `crates/gateway/tests/fixtures/402-envelope.json` (or a copy checked into `sdks/mcp/tests/fixtures/`) and asserts the MCP server's `parse402` and `createPaymentHeader` handle the exact shape. If the gateway's 402 envelope drifts, this test fails before release. *(Makes R5 mitigation real; currently fictional.)*
- **T1-J.** Fix `sdks/mcp/README.md` references: all `@solvela/mcp` strings → `@solvela/mcp-server`. Also update any internal docs (AGENTS.md, etc.) that reference the old name.
- **T1-K.** Session-state persistence (promoted from T5-B per rev 1.4 amendment). Create `~/.solvela/mcp-session.json` on first paid call; persist `{ session_spent_usdc, escrow_deposits_total_usdc, started_at, wallet_address }` atomically (write to tmp + rename) after each spend. Load on startup. Rationale: `SOLVELA_MAX_ESCROW_SESSION` (§4.5) must be enforceable from the moment `deposit_escrow` (T2-G) ships, not from Phase 5. Otherwise a server restart resets the cumulative cap — a restart-bypass attack surface on a payment-facing tool. Budget mutex (T1-H) serializes access. (~1 day, Phase 1 size rebumped to 2–3 days if T1-A.5 spike passes.)

**Phase dependency note:** T1-G completes before T2-B (host config generators) begins. The installer writes `SOLVELA_*` env vars; if `index.ts` still reads `RCR_*` as primary, installer output is silently broken. T1-K must complete before T2-G (deposit_escrow tool) ships.

**Verification:** S5, S6, S7, S8, S10 pass. Security review by subagent.

**README note to add:** "If your gateway runs in `dev_bypass_payment` mode, the MCP server will NOT send real signed transactions — the gateway accepts any payload. Intended for dev/CI. Set `SOLVELA_ALLOW_DEV_BYPASS=1` to silence the warning."

### Phase 2 — Host config installer + docs

**Size: 3–5 days.**

**Phase dependency:** T1-G (`RCR_*` → `SOLVELA_*` env var rename) MUST complete before T2-B (host config generators) begins. The installer writes `SOLVELA_*` env vars; if `index.ts` still reads `RCR_*` primary, installer output is silently broken.

Tasks:
- **T2-A.** Add `mcp install` subcommand to `crates/cli` (Rust). Args: `--host=<claude-code|cursor|claude-desktop|openclaw>`, `--scope=<user|project>` (default user), `--gateway-url=<url>` (default `https://api.solvela.ai`), `--wallet=<pubkey>` (optional; if omitted, uses the wallet from `solvela` config). **Dep guardrail:** any new dependencies added in this task must remain Windows-cross-compile-safe per the R8 addendum audit — no `build.rs`, no C deps, no native bindings that require platform-specific toolchains. If a feature genuinely requires such a dep, gate it behind `#[cfg(not(windows))]` with a graceful fallback, or surface for a separate decision before merging. Phase 4 T4-G.6 smoke-tests this binary on Windows via cross-compile.
- **T2-B.** Generators per host: produce exact JSON matching §10 appendix snippets. Claude Code: append to `.mcp.json` or user config. Cursor: append to `.cursor/mcp.json`. Claude Desktop: `claude_desktop_config.json`. OpenClaw: shell out to `openclaw mcp set solvela '<json>'`.
- **T2-C.** `--dry-run` flag prints the config without writing. `--diff` flag shows what would change.
- **T2-D.** Update `sdks/mcp/README.md` with the single-command install path for each host. Keep existing copy-paste snippets as fallback.
- **T2-E.** Add `--uninstall` support so users can cleanly remove Solvela from a host config.
- **T2-F.** Unit tests for JSON generation per host. Integration test: spawn each host's CLI in a temp dir and verify it loads the server.
- **T2-G.** Implement `deposit_escrow` MCP tool handler (approved per §4.5, previously unscheduled). Location: `sdks/mcp/src/index.ts`. Gate: if `SOLVELA_ESCROW_MODE !== 'enabled'`, omit the tool from the exported `TOOLS` array entirely (do not just refuse at call-time — reduces attack surface). Wire through `buildEscrowDeposit` from `@solvela/sdk` (available after T1-A.5 + T1-B). Enforce `SOLVELA_MAX_ESCROW_DEPOSIT` (default $5.00) per-call cap before signing. Enforce `SOLVELA_MAX_ESCROW_SESSION` (default $20.00) cumulative cap against `~/.solvela/mcp-session.json` (written by T1-K) — load, check, reject-or-deposit, update, persist. Cap check and persistence update run inside the T1-H budget mutex so concurrent `deposit_escrow` calls cannot both pass the check. Return `{ pda, expiry_slot, service_id, amount_usdc }` on success; return `SolvelaBudgetExceededError` when either cap fires. Phase dependency: T1-K must ship before T2-G.

**Verification:** S2, S3, S4, S11, S11.5 pass. (S11.5 now has a Phase 2 verification path — restart-bypass test is still added in Phase 5 per §5 since T1-K persistence makes the test meaningful across restarts.)

### Phase 3 — OpenClaw Provider Plugin

**Size: 5–7 days.** Approved per §4.2 Option B. Sequenced after Phase 2 so MCP server ships first and absorbs early-feedback churn before Provider Plugin starts.

Tasks:
- **T3-A.** New npm package `sdks/openclaw-provider/`. Manifest `openclaw.plugin.json` declares `providers: ["solvela"]` and `providerAuthEnvVars: { solvela: ["SOLANA_WALLET_KEY", "SOLANA_RPC_URL"] }`.
- **T3-B.** Implement `registerProvider` with `api: "openai-completions"`, `baseUrl: "https://api.solvela.ai/v1"`, and `wrapStreamFn` hook for per-call `PAYMENT-SIGNATURE` injection.
- **T3-C.** `wrapStreamFn` intercepts the outbound request, calls `createPaymentHeader` to produce the signed `PAYMENT-SIGNATURE` header (raw, not base64-reencoded), and injects it directly into the outbound request headers before the stream fires. This matches the pattern the MCP server uses — zero gateway changes required. The signed header is produced exactly as `createPaymentHeader` produces it for the MCP client's 402 retry path. **Partial-stream failure note:** if `wrapStreamFn` signs, injects, and the stream errors mid-response (network drop, provider timeout), direct-transfer payments are non-refundable — funds moved on-chain, no useful tokens returned. Escrow mode is the recommended default for Provider Plugin use precisely because the escrow claim only fires on a completed response; if the stream dies, the escrow refund path (T5-series in x402 spec) recovers the deposit. Document this trade-off prominently in `sdks/openclaw-provider/README.md` and default `SOLVELA_SIGNING_MODE=auto` (which prefers escrow when advertised) in the plugin's config schema.
- **T3-D.** *(Deleted — not needed.)* Gateway `Authorization: Bearer` change is not required. `wrapStreamFn` injects `PAYMENT-SIGNATURE` directly, which is what `middleware/x402.rs:38` already reads. The original T3-D (Bearer alias) would have touched three code paths (`middleware/x402.rs:38`, `routes/chat/mod.rs:142-144`, `routes/proxy.rs:110`) and conflated auth with payment authorization — avoided by using `wrapStreamFn`.
- **T3-E.** Model registry: generate from `config/models.toml` via codegen at build time; expose ~26 models.
- **T3-F.** `resolveDynamicModel` hook for routing profiles (`eco`, `auto`, `premium`, `free`).
- **T3-G.** Publish `@solvela/openclaw-provider@1.0.0` to npm.

**Verification:** S12 passes.

**Risk note on T3-C:** `wrapStreamFn` is now the primary (not fallback) injection path. The §7 R3 risk (hook not firing per-call) remains at low likelihood given the direct stream-intercept pattern.

### Phase 4 — Distribution

**Size: 5–7 days.** Additional ~4–8h over r1.2 for the npm meta-package scaffold (T4-G.4) per the R8 addendum's effort estimates. Gated on §4.6.

**Phase dependency (added rev 1.4):** T4-G.6 smoke-tests `solvela mcp install --host=claude-code` — that subcommand only exists after T2-A merges. T4-G.1–G.5 (pipeline scaffold) can proceed in parallel with Phase 2; **T4-G.6 must not run until Phase 2 is merged to main.** T4-A/B/C/D/E/F have no Phase 2 dependency.

Tasks:
- **T4-A.** Publish `@solvela/mcp-server@1.0.0` to npm with provenance (OIDC).
- **T4-B.** Submit MCP server manifest to Anthropic MCP Registry (`api.anthropic.com/mcp-registry/v0/servers`). Document-specialist subagent researches current submission process before this task starts.
- **T4-C.** Submit cursor.directory listing. PR to upstream repo with install metadata, description, screenshots.
- **T4-D.** OpenClaw docs PR — propose a "Pay-per-call LLM access via Solvela" entry in their integrations docs.
- **T4-E.** Launch post: `solvela.ai/blog` + HN Show + X thread + Solana Foundation grant update.
- **T4-F.** Publish `@solvela/openclaw-provider@1.0.0` after Phase 3 acceptance tests pass.
- **T4-G.** Cross-compile `solvela` CLI via `cargo-dist` and distribute through npm `optionalDependencies` using the turborepo/biome pattern. Replaces the former single-task cross-compile entry. Dependency audit (`2026-04-18-mcp-plugin-plan-review-r8-addendum.md`) confirms the workspace is already Windows-cross-compile-safe — no `solana-sdk`, `rustls-tls` only, no `build.rs`. Sub-tasks:
  - **T4-G.1.** Add `[workspace.metadata.dist]` to workspace `Cargo.toml` with targets `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, plus `installers = ["shell", "powershell"]`. (Linux ARM64 added in rev 1.4 — cheap now via `cargo-dist`; covers Graviton CI runners and ARM dev machines.)
  - **T4-G.2.** Run `cargo dist init`; commit generated `.github/workflows/release.yml`.
  - **T4-G.3.** Ensure `crates/cli/Cargo.toml` exposes the `solvela` binary via `[[bin]]`; verify `cargo dist plan` lists all five artifacts.
  - **T4-G.4.** Create `sdks/cli-npm/` package scaffold. Meta-package `@solvela/cli` declares `optionalDependencies` for five platform packages: `@solvela/cli-linux-x64`, `@solvela/cli-linux-arm64`, `@solvela/cli-win32-x64`, `@solvela/cli-darwin-x64`, `@solvela/cli-darwin-arm64`. Each platform package contains the prebuilt binary and a `bin` entry. Main package ships a ~50-line JS shim that detects `process.platform + process.arch` and execs the binary. Use biome's `@biomejs/cli` as layout template: https://github.com/biomejs/biome/tree/main/packages/%40biomejs/cli — copy `index.js` shim logic near-verbatim (platform/arch detection + exec), adjust package names.
  - **T4-G.4.5.** **(Added rev 1.4 — release → npm publish automation glue.)** Add a GH Actions workflow (or post-job in `release.yml`) that fires on the tagged release: (a) downloads the five platform binaries from the GH Release artifacts, (b) copies each into its respective `@solvela/cli-<platform>-<arch>` package directory under `sdks/cli-npm/`, (c) runs `npm publish --provenance --access=public` for all six packages (`@solvela/cli` meta + 5 platform packages) in the correct order (platform packages first, meta-package last, so the meta-package's `optionalDependencies` resolve at publish time). Reference biome's `.github/workflows/release_cli.yml` as template. **OIDC provenance is mandatory for all six packages** (not just `@solvela/mcp-server` from T4-A) — configure the workflow with `permissions: { id-token: write, contents: read }` and `NPM_CONFIG_PROVENANCE=true`. On partial-publish failure (e.g. 4/5 platforms succeed), the workflow must fail the entire release and tag the version as `-aborted` so humans re-run cleanly; never ship a meta-package whose `optionalDependencies` reference a missing platform version.
  - **T4-G.5.** First tagged release `v1.0.0`; verify all five binaries upload to GH Releases and `@solvela/cli` + platform packages auto-publish to npm with provenance attestation visible on npmjs.com.
  - **T4-G.6.** Smoke test (**gated on Phase 2 merge per Phase 4 dependency note**): `npm i -g @solvela/cli` on Windows → run `solvela mcp install --host=claude-code` → verify it writes valid MCP config. Repeat on Linux x64/arm64 and macOS (x64 + arm64).

**Verification:** S14, S15 pass. Metrics: npm weekly downloads, registry listing analytics, blog post traffic.

### Phase 5 — Budget + session hardening

**Size: 1–2 days.** (Reduced from 2–3 days in rev 1.4 — T5-B promoted to Phase 1 as T1-K.)

Tasks:
- **T5-C.** Add `spending --reset` tool to clear the persisted file (`~/.solvela/mcp-session.json`, written by T1-K) when the user wants a fresh session.
- **T5-D.** `--budget=<usdc>` flag on `solvela mcp install` writes `SOLVELA_SESSION_BUDGET` into the host config.
- **T5-E.** Restart-bypass regression test: start MCP server, call `deposit_escrow` 4× $4 ($16 cumulative), kill the process, restart, attempt a 5th $4 deposit. Must be rejected (session total would be $20 + $4 = exceeds `SOLVELA_MAX_ESCROW_SESSION` default of $20). Validates that T1-K persistence survives restart and the cap is not a restart-bypass. (Test added per rev 1.4 — not redundant with S11.5's in-session test since that one doesn't cover the restart case.)

*(T5-A — concurrency-safe budget mutex — was promoted to Phase 1 as T1-H in rev 1.2.)*
*(T5-B — session spend persistence — was promoted to Phase 1 as T1-K in rev 1.4 to close the restart-bypass window on `deposit_escrow`.)*

**Verification:** T5-E passes (restart-bypass regression covered).

### Phase 6 — Hallway test + iteration

**Size: ongoing.**

Tasks:
- **T6-A.** Recruit one external tester per host (Claude Code, Cursor, OpenClaw). No overlap with prior testers from the AI SDK plan.
- **T6-B.** Cold-install test: README only, no agent support. Log first sticky point for each tester.
- **T6-C.** Incorporate fixes into 1.0.x patch releases. Post-mortem notes land in `docs/strategy/`.

**Verification:** S13 passes.

---

## 7. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | `@solvela/sdk` ESM/CJS interop fails for MCP server (ESM-only) or OpenClaw Provider Plugin (`sdks/typescript/package.json` has no `exports` map; `x402.ts` uses internal `require()` calls). | **high** | high | T1-A.5 is a concrete Phase 1 pre-T1-B spike that resolves this before any signer work lands. If spike fails: dual-publish `@solvela/sdk` via `tsup`, or extract `@solvela/signer-core` coordinating with sibling plan §3.8. Do not inline the signer. |
| R2 | Anthropic MCP Registry submission denied or delayed. | medium | medium | Phase 4 has npm-only fallback. Registry listing is accelerator, not blocker. |
| R3 | OpenClaw's `wrapStreamFn` hook doesn't fire per-call — only once per session or not at all. | low | medium | `wrapStreamFn` is now the primary injection path (T3-C); it intercepts each outbound stream. If it proves not per-call, document the limitation and fall back to `prepareRuntimeAuth` returning a signed header value. |
| R4 | Hot key on disk gets exfiltrated from a user's machine; they blame Solvela. | low | high | Escrow-first default means hot key only signs small deposit txs with explicit amounts, not blanket-authorized transfers. Docs emphasize escrow mode. V1.1 adds hardware wallet adapter. |
| R5 | Gateway's 402 response shape changes during implementation. | low | high | T1-I adds a real contract test consuming `crates/gateway/tests/fixtures/402-envelope.json`. Any 402 envelope drift fails this test before release. |
| R6 | Competitor ships a Solana x402 MCP server first. | medium | medium | Phase 1 has 1–2 day size; execute fast. Publish an opinionated README that emphasizes escrow + Rust gateway + multi-host support as differentiation. |
| R7 | Model-controlled `deposit_escrow` tool drains the agent's wallet. | medium (if enabled) | high | §4.5 gates the tool behind `SOLVELA_ESCROW_MODE=enabled`. Two-layer cap: `SOLVELA_MAX_ESCROW_DEPOSIT` ($5 per call) AND `SOLVELA_MAX_ESCROW_SESSION` ($20 cumulative per session) enforced against `~/.solvela/mcp-session.json`. An adversarial loop cannot exceed the session cap regardless of call count. |
| R8 | `solvela` Rust CLI doesn't ship Windows binaries; Windows users can't use `solvela mcp install`. | low | low | Cross-compile the Rust CLI via `cargo-dist` and distribute through npm `optionalDependencies` using the turborepo/biome pattern. Dependency audit (addendum §"Key evidence") confirms the workspace is already Windows-cross-compile-safe — no `solana-sdk`, `rustls-tls` only, no `build.rs`. Effort: ~16h to first release; ~30 min per subsequent release. |

---

## 8. Out of scope

- Multi-wallet signer backends (hardware, Phantom deeplink, MPC). V2.
- Browser-side MCP. Spec only supports stdio and HTTP; browser MCP is not a thing yet.
- OpenClaw Skill package (markdown guidance on when to use Solvela). Separate plan.
- Fiat / card payment MCP tools. Against CLAUDE.md rule 6.
- Direct Solana RPC health probing from `wallet_status` in the MCP server. Deferred to V1.1; V1.0 trusts the gateway's `/health` response. See §5 S9.
- Tool coverage beyond the existing 5 + conditional `deposit_escrow`. A `services_list` tool to surface the x402 marketplace is a follow-up.
- Telemetry from installed MCP servers back to Solvela (privacy concern; explicit opt-in only).
- Automated Cursor/Claude Code UI tests. Manual QA in Phase 2.
- Pure JavaScript reimplementation of the installer generators (previously considered as an `@solvela/mcp-install` wrapper). Replaced by the hybrid C design in §4.4 / T4-G: a thin npm meta-package that execs the Rust binary, with zero JS-side generator logic. No drift class exists.

---

## 9. Coordination with sibling plans

### With `2026-04-16-vercel-ai-sdk-provider-plan.md`

| Shared concern | This plan | Sibling plan | Coordination |
|---|---|---|---|
| Signer implementation | Imports `createPaymentHeader` from `@solvela/sdk` (v1.0). T1-A.5 resolves ESM/CJS import path before use. | Defines `SolvelaWalletAdapter` interface + `createLocalWalletAdapter` reference impl. | If sibling plan extracts `@solvela/signer-core` (its §3.8 Option B), this plan's V1.1 migrates to it. Documented in HANDOFF after both V1s ship. |
| Wallet adapter interface | N/A V1 (uses direct env var). | First-class concept. | V1.1 of this plan can accept a `SolvelaWalletAdapter` instance via a programmatic start API, enabling hardware-wallet users to share the same adapter. |
| Gateway 402 contract | Contract consumer. T1-I adds real contract test against `crates/gateway/tests/fixtures/402-envelope.json`. | Contract consumer (`sdks/ai-sdk-provider/tests/unit/parse-402.test.ts`, `adapter-contract.test.ts`). | **Ownership (rev 1.4):** Gateway team owns `crates/gateway/tests/fixtures/402-envelope.json`. Any gateway PR that changes the 402 response shape must update the fixture in the same PR. Both consumer plans run their contract tests against gateway HEAD in CI — a fixture change that breaks either consumer fails CI on the gateway PR, forcing cross-plan coordination at the source rather than downstream. |
| ESM/CJS interop | T1-A.5 spike resolves this in Phase 1. Phase 0 gates Phase 1 on the spike result or the sibling plan's §3.8 resolution. | Phase 1 spike (§3.8). | T1-A.5 result is documented in `.omc/plans/open-questions.md`. Both plans pause if all three Phase 0 paths fail. |
| `@solvela/mcp-server` stability | Consumer of `@solvela/sdk` signing. Must not be broken by `@solvela/sdk` restructuring. | Out-of-scope section excludes edits to `@solvela/mcp-server`. | Courtesy note: sibling plan's next revision should acknowledge `@solvela/mcp-server` as a signing-dependency consumer. This plan does not edit the sibling plan file. |

### With gateway (`crates/gateway`)

Phase 3 (OpenClaw Provider Plugin) uses `wrapStreamFn` to inject `PAYMENT-SIGNATURE` directly — no gateway changes required. The original T3-D (Bearer alias to `middleware/x402.rs`) is deleted; see §6 Phase 3 T3-D note. No gateway coordination plan is needed for this plan's scope.

---

## 10. Appendix — Verified host config snippets

### 10.1 Claude Code

```jsonc
// .mcp.json (project-scoped) OR user config via `claude mcp add`
{
  "mcpServers": {
    "solvela": {
      "command": "npx",
      "args": ["-y", "@solvela/mcp-server"],
      "env": {
        "SOLVELA_API_URL": "https://api.solvela.ai",
        "SOLVELA_SESSION_BUDGET": "1.00",
        "SOLVELA_SIGNING_MODE": "auto",
        "SOLANA_WALLET_ADDRESS": "<pubkey>",
        "SOLANA_WALLET_KEY": "<base58 keypair secret>",
        "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com"
      }
    }
  }
}
```

CLI equivalent: `claude mcp add solvela -s user -- npx -y @solvela/mcp-server` then `claude mcp set-env solvela SOLANA_WALLET_KEY=<key>` etc.

### 10.2 Cursor

```jsonc
// .cursor/mcp.json (project) or ~/.cursor/mcp.json (global)
{
  "mcpServers": {
    "solvela": {
      "type": "stdio",
      "command": "npx",
      "args": ["-y", "@solvela/mcp-server"],
      "env": {
        "SOLVELA_API_URL": "https://api.solvela.ai",
        "SOLVELA_SESSION_BUDGET": "1.00",
        "SOLVELA_SIGNING_MODE": "auto",
        "SOLANA_WALLET_ADDRESS": "${env:SOLANA_WALLET_ADDRESS}",
        "SOLANA_WALLET_KEY": "${env:SOLANA_WALLET_KEY}",
        "SOLANA_RPC_URL": "${env:SOLANA_RPC_URL}"
      },
      "envFile": "${userHome}/.solvela/env"
    }
  }
}
```

One-click install URL (to be minted by Phase 4 submission):
`cursor://anysphere.cursor-mcp/install?name=solvela&config=<base64-encoded-above>`

### 10.3 Claude Desktop

Same schema as Claude Code `.mcp.json`, written to `claude_desktop_config.json` (platform-specific path: `~/Library/Application Support/Claude/` on macOS, `%APPDATA%\Claude\` on Windows, `~/.config/Claude/` on Linux).

### 10.4 OpenClaw (MCP)

```bash
openclaw mcp set solvela '{
  "command": "npx",
  "args": ["-y", "@solvela/mcp-server"],
  "env": {
    "SOLVELA_API_URL": "https://api.solvela.ai",
    "SOLVELA_SIGNING_MODE": "auto",
    "SOLANA_WALLET_KEY": "<base58 keypair secret>",
    "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com"
  }
}'
```

Stored at `~/.openclaw/openclaw.json` under `mcp.servers.solvela`.

### 10.5 OpenClaw (Provider Plugin)

```jsonc
// sdks/openclaw-provider/openclaw.plugin.json
{
  "id": "solvela",
  "providers": ["solvela"],
  "providerAuthEnvVars": {
    "solvela": ["SOLANA_WALLET_KEY", "SOLANA_RPC_URL"]
  },
  "providerAuthChoices": [
    {
      "provider": "solvela",
      "method": "api-key",
      "cliFlag": "--solana-wallet-key",
      "envVar": "SOLANA_WALLET_KEY"
    }
  ],
  "configSchema": {
    "type": "object",
    "properties": {
      "apiUrl": { "type": "string", "default": "https://api.solvela.ai" },
      "signingMode": { "type": "string", "enum": ["auto", "escrow", "direct"], "default": "auto" }
    }
  }
}
```

```typescript
// sdks/openclaw-provider/src/index.ts
import { createPaymentHeader } from '@solvela/sdk';

export default function register(api: OpenClawApi) {
  api.registerProvider({
    id: 'solvela',
    label: 'Solvela (USDC-gated multi-provider gateway)',
    docsPath: 'https://docs.solvela.ai/openclaw',
    envVars: ['SOLANA_WALLET_KEY', 'SOLANA_RPC_URL'],
    auth: [{ method: 'api-key', envVar: 'SOLANA_WALLET_KEY' }],
    catalog: {
      order: 'late',
      run: async (ctx) => ({
        provider: {
          baseUrl: 'https://api.solvela.ai/v1',
          apiKey: 'deferred',  // filled by wrapStreamFn
          api: 'openai-completions',
          models: SOLVELA_MODELS,  // codegen from config/models.toml
        },
      }),
    },
    wrapStreamFn: async (request, next) => {
      // Inject PAYMENT-SIGNATURE directly into the outbound request before the stream fires.
      // This matches what middleware/x402.rs:38 reads — no gateway changes required.
      const header = await createPaymentHeader(
        request.paymentInfo,
        request.url,
        process.env.SOLANA_WALLET_KEY,
        request.body,
      );
      request.headers['PAYMENT-SIGNATURE'] = header;
      return next(request);
    },
  });
}
```

---

## 11. Change log

- **1.0 (2026-04-18)** — initial draft. Research folded into §2 per advisor feedback. Six scope decisions surfaced as §4.1–4.6. Coordination with sibling AI SDK plan codified in §9. Pending user approval of §4 decisions.
- **1.1 (2026-04-18)** — all six §4 decisions approved with planner-recommended options. Locked: auto signing mode (C), MCP + Provider Plugin for OpenClaw (B), `@solvela/sdk` dependency for v1.0 (A), Rust CLI installer + npm wrapper for Windows (B), `deposit_escrow` tool with cap (A), full launch slate distribution (D). Phase 3 de-conditionalized; Phase 4 adds T4-G (`@solvela/mcp-install` npm wrapper for Windows per R8 mitigation). Status advanced to "Approved for execution pending one final user review pass."
- **1.2 (2026-04-18)** — all 13 review amendments applied. Key changes: (1) T3-D deleted; T3-C rewritten to use `wrapStreamFn` for `PAYMENT-SIGNATURE` injection — no gateway changes required; §9 gateway coordination bullet removed; §10.5 code sample updated to `wrapStreamFn` pattern. (2) T1-A.5 added (ESM/CJS spike + exports-map PR, must precede T1-B). (3) Explicit T1-G → T2-B phase dependency noted in Phase 1 and Phase 2. (4) Windows strategy locked by user decision: T4-G replaced with `cargo-dist` cross-compile release pipeline (windows/linux/darwin binaries); `@solvela/mcp-install` npm wrapper deleted from scope; §4.4, §7 R8, §8, §2.2 updated accordingly; Phase 4 size bumped to 4–6 days. (5) `SOLVELA_MAX_ESCROW_SESSION` ($20 cumulative cap) added to §4.5; S11.5 criterion added; §7 R7 mitigation updated to reflect two-layer cap. (6) T5-A promoted to Phase 1 as T1-H (budget mutex, ~10 LOC); S10 updated to match; Phase 5 retains T5-B/C/D only. (7) Package name discrepancy resolved: `@solvela/mcp-server` is canonical; T1-J added to fix README. (8) S9 softened to reflect V1.0 trusts gateway `/health`; direct RPC probing deferred to V1.1 and added to §8 out-of-scope. (9) T1-I added (real contract test consuming gateway 402 fixture); §7 R5 updated. (10) T1-D expanded with `dev_bypass_payment` startup warn + README note. (11) §7 R1 re-rated from `medium` to `high`; mitigation updated to reference T1-A.5. (12) Phase 0 cross-plan coordination gate added. (13) T1-G wording changed to silent/patch-window deprecation, no warning log. Status advanced to "Ready for execution."
- **1.3 (2026-04-18)** — adopted hybrid C Windows distribution per R8 addendum (`2026-04-18-mcp-plugin-plan-review-r8-addendum.md`). T4-G expanded from single-task cross-compile into six sub-tasks T4-G.1–T4-G.6 covering `cargo-dist` config + npm `optionalDependencies` meta-package following turborepo/biome pattern. §7 R8 mitigation rewritten; likelihood dropped to low (dep tree verified Windows-safe per addendum). §4.4 approved-option updated from pure B to Hybrid C. §2.2 table note + §8 out-of-scope entry updated to reflect npm meta-package as primary install path. Phase 4 size bumped from 4–6 days to 5–7 days (+4–8h for optional-deps scaffold per addendum effort estimates). No other §4 decisions reopened. All prior amendments from r1.2 preserved unchanged.
- **1.4 (2026-04-18)** — rev 1.3 review amendments applied (`2026-04-18-mcp-plugin-plan-review-r1.3.md`). **P0 fixes:** (1) T2-G added — `deposit_escrow` tool handler implementation was approved in §4.5 with S11/S11.5/R7 but had no T-task anywhere; rev 1.4 wires it to `buildEscrowDeposit` from `@solvela/sdk`, gates on `SOLVELA_ESCROW_MODE`, enforces both caps, and runs inside the T1-H budget mutex. (2) T5-B promoted to Phase 1 as T1-K — `~/.solvela/mcp-session.json` persistence now ships alongside the escrow tool rather than waiting until Phase 5, closing the restart-bypass window on `SOLVELA_MAX_ESCROW_SESSION`. **P1 fixes:** (3) §3 Access/credentials adds `@solvela` npm org scope check covering all 6 packages (`@solvela/cli` + 5 platform packages). (4) T4-G.4.5 added — GH Actions workflow that downloads release binaries, copies to platform package dirs, and runs `npm publish --provenance` atomically; OIDC provenance is mandatory for all 6 packages. (5) Phase 4 → Phase 2 dependency made explicit — T4-G.6 smoke test gated on Phase 2 merge; T4-G.1–G.5 pipeline scaffold can proceed in parallel. (6) §9 gateway-fixture ownership documented — gateway team owns `crates/gateway/tests/fixtures/402-envelope.json`; both consumer plans run contract tests against gateway HEAD in CI. **P2 fixes:** (7) stale `(r1.2 approved option)` parenthetical at §4.4 Option B updated to note supersession. (8) biome template link in T4-G.4 made specific (`@biomejs/cli`). (9) `aarch64-unknown-linux-gnu` target added to T4-G.1 (Graviton + ARM dev); platform package added to T4-G.4 + T4-G.4.5. (10) OIDC provenance scope made explicit across T4-A and T4-G. (11) T2-A dep guardrail added ("Windows-cross-compile-safe per R8 addendum audit"). (12) T3-C partial-stream failure note added — escrow mode recommended as Provider Plugin default because direct-transfer mid-stream failures are non-refundable. **Phase resizing:** Phase 1 2–3 days (was 1–2) absorbing T1-K; Phase 5 1–2 days (was 2–3) after T5-B promotion. New T5-E restart-bypass regression test replaces in-Phase-5 S11.5 coverage. No §4 decisions reopened.
