# Changelog

All notable changes to Solvela (formerly RustyClawRouter), in reverse chronological order.

## 2026-05-09 — Whole-repo security review pass

Internal review-pass across all 6 workspace crates plus the standalone Anchor escrow program. 18 must-ship PRs ([#182](https://github.com/solvela-ai/solvela/pull/182)–[#199](https://github.com/solvela-ai/solvela/pull/199)) landed on `main`; cleanup-tier wave 1 ([#200](https://github.com/solvela-ai/solvela/pull/200)) open with 8 deferred MEDIUMs. All HIGH/CRITICAL findings closed in source. The deployed mainnet escrow bytecode does not yet include the 2026-05-09 escrow source fixes — a redeploy is tracked in `STATUS.md` follow-ups. `getProgramAccounts` returned zero accounts when the new findings landed — both newly-discovered escrow footguns were latent and unexploited.

### x402 protocol library — 7 PRs ([#182–#188](https://github.com/solvela-ai/solvela/pull/182))

Solana verification, fee-payer pool, nonce pool, and escrow-claim queue hardening. Headline fixes: sysvar typo + verifier provider check ([#182](https://github.com/solvela-ai/solvela/pull/182), 2 CRIT), atomic claim queue with `FOR UPDATE SKIP LOCKED` ([#183](https://github.com/solvela-ai/solvela/pull/183), 2 HIGH + 1 MED), fee-payer counter race + zero secret decode buffer ([#184](https://github.com/solvela-ai/solvela/pull/184), 1 HIGH + 1 MED), tightened `0x1` `InsufficientFunds` match ([#185](https://github.com/solvela-ai/solvela/pull/185)), confirm-loop dedup ([#186](https://github.com/solvela-ai/solvela/pull/186)), nonce-pool env reader + on-chain authority check ([#187](https://github.com/solvela-ai/solvela/pull/187), 1 HIGH + 1 MED), and `cargo audit` rationale for the unreachable `RUSTSEC-2023-0071` (`rsa` via `sqlx-mysql`, [#188](https://github.com/solvela-ai/solvela/pull/188)).

### Gateway — 6 PRs ([#189–#194](https://github.com/solvela-ai/solvela/pull/189), G1–G6)

Surface-by-surface remediation across the Axum service:

- **G1 — SSRF cluster** ([#189](https://github.com/solvela-ai/solvela/pull/189), 3 CRIT + 1 HIGH): closed 4 SSRF paths around the proxy, services registry, and config bootstrap. DNS pinning, CGNAT exclusion, redirect policy.
- **G2 — Info leaks** ([#190](https://github.com/solvela-ai/solvela/pull/190), 4 HIGH + 1 MED): sanitised 5 leak surfaces in error variants and structured logs (Solana RPC URL, recipient wallet, raw verifier errors).
- **G3 — Financial correctness** ([#191](https://github.com/solvela-ai/solvela/pull/191), 3 HIGH): 3 silent revenue gaps in cost computation, cap-to-request-limits, and provider attribution.
- **G4 — Budget enforcement** ([#192](https://github.com/solvela-ai/solvela/pull/192), 2 HIGH + 1 MED): atomic Lua `INCRBYFLOAT` for Redis budget reservation, team-budget fail-closed lookup, shared default-policy resolution.
- **G5 — Multi-tenant authz cluster** ([#193](https://github.com/solvela-ai/solvela/pull/193), 6 HIGH): role-cap tightening (only Owner can promote Admin), 32-byte API key entropy + UUID-derived prefix, org-guarded queries on every team/key/audit/budget endpoint, audit-actor field, info-leak removals on `/nonce` and `/services`.
- **G6 — Reliability bundle** ([#194](https://github.com/solvela-ai/solvela/pull/194), 1 CRIT + 2 HIGH): build-error propagation in `balance_monitor`, single-probe gating in the provider half-open circuit-breaker, out-of-band rate-limit emergency-cleanup helper.

### Router — 1 PR ([#195](https://github.com/solvela-ai/solvela/pull/195), R1)

Broken `haiku` alias (was pointing at deprecated 3-5 ID), added load-time pricing validation (rejects NaN/Inf/negative/duplicate canonical keys at registry construction), and a cross-check test that loads production `models.toml` to prevent regression. 1 HIGH + 2 MED.

### Protocol — 1 PR ([#196](https://github.com/solvela-ai/solvela/pull/196), P1)

`Role` enum gained a `#[serde(other)] Unknown` catch-all so a future provider-introduced role value lands cleanly instead of failing the whole message parse. `Usage::new(prompt, completion)` constructor uses `saturating_add` for `total_tokens`. `#[serde(deny_unknown_fields)]` applied to every x402 payment-payload type — also disambiguates the previously-untagged `PayloadData` union. Multimodal `ContentPart` flagged unreachable in `vision.rs` doc; future cross-cutting PR will plumb it through. 3 HIGH + 2 chore.

### CLI — 2 PRs ([#197 + #199](https://github.com/solvela-ai/solvela/pull/197), CLI1 + CLI2)

- **CLI1**: wallet `fs::write` + chmod race fixed via shared atomic-write helper (chmod-on-tempfile-FD-before-rename); 180s/30s timeouts on `chat::run` and `recover::run` HTTP clients (were hanging indefinitely on stalled gateway/RPC); saturating math + 10,000-slot cap on the escrow expiry-slot computation in both `chat.rs` and `loadtest/payment.rs` (raw `u64` multiply was wrappable to a near-zero `expiry_slot`); URL-scheme validation on the global `--api-url` (was accepting `file://`); `validate_wallet` re-applied to the disk-loaded wallet address before passing to `claude mcp add`. 3 CRIT + 2 HIGH.
- **CLI2**: typed `WalletData { address, private_key: SecretString }` replaces raw `serde_json::Value` return from `load_wallet()` so any future `{:?}`-format of the wallet redacts the key automatically; `Zeroizing<>` wraps on the derived seed/keypair byte arrays in three customer-facing key-handling paths so process-memory dumps don't yield the raw secret. 2 HIGH.

### Escrow program — 1 PR ([#198](https://github.com/solvela-ai/solvela/pull/198), ESCROW1)

Two CRIT-class footguns closed; both result in the same blast radius (free service for the agent, no payment to the provider) via different mechanisms. Mainnet bytecode redeploy pending — deployed bytecode does not yet include these fixes, but `getProgramAccounts` returns zero accounts so the impact surface is zero deposits.

- **Vault dust stuffing** — `spl-token`'s `CloseAccount` aborts on any non-zero balance. Vault ATA address is publicly derivable and SPL transfers don't require recipient signature, so anyone (incl. the agent) could transfer 1 dust unit into the vault and brick the provider's claim path. After expiry the agent refunds full amount + dust at zero net cost. **Fix:** reload + drain vault residual to agent before `close_account`.
- **Instant-refund window** — `expiry_slot > now` was the only lower bound. Agent could deposit with `expiry_slot = now + 1`, gateway delivers response, gateway's claim tx can't land before slot moves past expiry, claim fails, agent refunds. **Fix (two layers):** new `MIN_EXPIRY_BUFFER = 50` slots on-chain (`programs/escrow/src/lib.rs`); off-chain verifier in `crates/x402/src/escrow/verifier.rs` parses the previously-skipped `expiry_slot` field, fetches current slot via new `solana_rpc::get_current_slot` helper, and rejects deposits with `buffer < 50`.

### Cleanup wave 1 — 1 PR ([#200](https://github.com/solvela-ai/solvela/pull/200), open)

8 deferred MEDIUMs across CLI and escrow: `models.rs` HTTP timeout, `solana_tx.rs` USDC-MINT-parse `.expect()` removed, `recover.rs` swallowed RPC errors now `tracing::warn!`'d, `chat.rs` 402-error-chain clarification, `mcp/mod.rs` cwd-failure bail, `Transfer` → `TransferChecked` across all 5 escrow transfer sites (defense-in-depth over existing mint pinning), and a deployment checklist + `cargo build-sbf` fallback note in `programs/escrow/AGENTS.md`. None security-critical.

### Verification

- Every must-ship PR was bot-approved (`solvela-per`) and squash-merged in number order; #199 was rebased + force-pushed once to resolve a `mcp/mod.rs` conflict with #197's `validate_wallet` wrap.
- Per-crate test counts at HEAD: cli 189, x402 132, gateway 497, router 19, protocol 18; escrow 23 integration + 8 unit tests pass via `cargo build-sbf && cargo test --features sbf`.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` both clean (escrow excluded from workspace per its standalone Cargo.toml).

## 2026-05-08 — Escrow program hardening

Pre-launch security review of `programs/escrow/` surfaced two latent settlement-path footguns. Both fixed in source ([PR #160](https://github.com/solvela-ai/solvela/pull/160)) and the bytecode redeployed to mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` on slot 418438627 (tx `2Rp69yKfyxeeawrFRPZbLma9NytxEXSz8PeWPdNYzWm4e3Fr2xihP3T23PiwDB9xYbnUwN8kmsnrgBKjP2dbBbuE`). `getProgramAccounts` returned zero accounts at the time of redeployment — both flaws were latent and unexploited.

### Findings (proactively patched)

- **Claim-vs-refund race at the expiry boundary** — once `slot >= expiry_slot`, both `claim` and `refund` were simultaneously valid. An adversarial provider could race the agent at the deadline and capture the deposit. New guard requires `Clock::slot < expiry_slot` for `claim`.
- **Refund deadlock when the agent's USDC ATA had been closed** — agents who closed their ATA after deposit (a normal post-deposit move that reclaims rent) would find both `refund` and `claim`'s refund leg failing. The funds would have been permanently stuck in the vault PDA. Both instructions now use `init_if_needed` so the agent's ATA is recreated on the fly when needed.

### Secondary hardening (same PR)

- `USDC_MINT` feature-gated (`mainnet` selects `EPjFW…`, default selects devnet). Same source compiles for any cluster without code edits.
- `deposit` rejects `provider == agent` and `provider == Pubkey::default()` (catches misconfigured client calls before funds are locked).
- `expiry_slot` capped at `now + MAX_ESCROW_SLOTS` (~1 day at 400ms/slot).
- `claim`'s `agent` constraint switched from `SystemAccount` → `UncheckedAccount` (unblocks PDA-/program-owned agents in the future).
- `ClaimEvent` gains a `deposited` field for indexer disambiguation.
- `refund` rejects `vault_amount == 0`; `claim` uses `checked_sub` for the refund computation.
- 7 new tests added (5 integration covering claim-after-expiry, ATA-closed recovery, self-provider rejection, excessive-expiry rejection; 2 unit covering the new event field and the slot ceiling).

### Verification

- `cargo build-sbf -- --features mainnet` produced bytecode hash `2293edfce3a0e7b1b0332bc32d9f7a9e58105a2a3f825ee05d8a4f7fbf57bf26`, matching the on-chain dump truncated to source size.
- 27/27 tests pass (8 unit + 19 integration) with `--features sbf,mainnet`.
- Tracking issues #161 (redeploy ceremony — completed), #162 (dedicated escrow CI workflow — open), #155/#156 (anchor 0.31→1.0 migration — open).

## 2026-04-29 — Security audit + hardening

End-to-end security audit across the gateway and all four SDK repos (`solvela-python`, `solvela-ts`, `solvela-go`, `solvela-client`). 5 GitHub Security Advisories drafted on `solvela-ai/solvela`, 8 hardening issues filed publicly, and 4 SDK security-fix PRs landed.

### Gateway (PR [#74](https://github.com/solvela-ai/solvela/pull/74) — merged)

- **GHSA-wc9q-wc6q-gwmq (HIGH)** — `replay_set` mutex panic-poison no longer takes down `/v1/chat/completions`. Recovered via `PoisonError::into_inner()`.
- **GHSA-6ggq-cvwx-4f67 (HIGH)** — Rate-limit key switched from client-supplied `pay_to` to the payer wallet derived from the signed transaction. An attacker can no longer escape into a fresh per-client bucket by varying `pay_to`.
- **GHSA-cgqx-mg48-949v (HIGH)** — `GatewayError::InvalidPayment` and related variants no longer echo attacker-controlled payment fields or raw verifier errors. The internal Solana RPC URL and the recipient wallet stay server-side.

Two CRITICAL/HIGH advisories deferred to a follow-up PR (durable-nonce replay [GHSA-fq3f-c8p7-873f], f64 budget bypass [GHSA-86cr-h3rx-vj6j]).

### SDK security release — Python v0.1.0, TypeScript v0.2.0, Go v0.1.0, Client (Rust) v0.2.0

Cross-SDK fixes (applied uniformly to all 4):

- **Default `gateway_url` flipped to `https://api.solvela.ai`**. Plaintext `http://` is rejected for non-loopback hosts.
- **`network` and `asset` validation** added to the 402 handshake. A malicious or MITM'd gateway can no longer redirect payments to a different SPL token or non-mainnet network.
- **`max_payment_amount` defaults to 10 USDC** (`10_000_000` atomic). Wallet-drain protection is now opt-out.

Per-SDK highlights:

- **Python**: BIP39 mnemonic no longer echoed in `WalletError` (CRITICAL — exfil risk via Sentry/`logging.exception`); `chat_stream` now honors the signer via a non-streaming 402 pre-flight; RPC blockhash error path no longer leaks raw response body; `solders` upper-bound pinned.
- **TypeScript**: `parseAtomicAmount` rejects `NaN`/`Infinity`/`<=0` before any cap check; `Wallet.signTransaction()` replaces public `getKeypair()`; SSE chunks `try/catch` instead of crashing the generator; gateway error strings sanitized for log injection; `npm audit` added to CI.
- **Go**: `NewClient` now returns `(*SolvelaClient, error)` so config validation can fail closed; `Wallet.Sign(msg)` replaces public `PrivateKey()`; `ChatStream` runs the 402 handshake; `DeriveSessionIDWithSalt` mixes per-client `crypto/rand` salt; `FetchModels` body capped via `io.LimitReader`.
- **Client (Rust)**: `Wallet` stores `Zeroizing<[u8; 64]>` for real zero-on-drop (the previous `Drop` impl zeroed a stack copy); `ahash` replaces `DefaultHasher` for cache keys; `BudgetExceeded` error tracks cumulative spend across retries; new CI workflow runs fmt + clippy + test + `cargo audit`.

### Repository hardening

- **Branch protection on `main`** — 1 PR approval, 4 required CI checks (Rust fmt/clippy/test, Smoke test, Security audit, Docker build), branches must be up-to-date, force-push/delete blocked.
- **Hourly deploy-staleness check** (`.github/workflows/deploy-staleness-check.yml`) — opens a tracking issue if production sha lags `main` by more than 1 hour. Closes the gap that caused a 10-day silent stall.
- **`www.solvela.ai`** added as a 308 redirect to the apex on Vercel.
- **GitHub Releases** — first releases tagged on the main repo (`v0.1.1`, `solvela-x402-v0.1.2`) and on all four SDK repos.

## 2026-04-27 — v0.1.1 Patch Release

- **Fixed `solvela models` showing `?` for every price**: CLI was reading `pricing.input_cost_per_million` / `pricing.output_cost_per_million` but the gateway returns `pricing.input_per_million` / `pricing.output_per_million`. Prices now render correctly (e.g. `$0.42`). Test mock updated to match the real production response shape.
- **Fixed `solvela health` printing `Version: unknown`**: Gateway `/health` returns only `{"status":"ok"}` — no version field. Removed the `Version:` line from health output rather than printing a misleading "unknown".

## 2026-04-11 — Solvela Rebrand

- **Rebranded from RustyClawRouter to Solvela**: All crate names, CLI binary, env var prefixes, HTTP headers, SDK packages, infrastructure config, documentation, and dashboard UI updated. RustyClaw.ai remains the separate trading terminal product.
- **Crate renames**: `rustyclaw-protocol` -> `solvela-protocol`, `rcr-cli` -> `solvela-cli`. Binary: `rustyclawrouter` -> `solvela-gateway`, CLI: `rcr` -> `solvela`.
- **Env var prefix**: `SOLVELA_` prefix (legacy `RCR_` accepted with deprecation warning).
- **HTTP headers**: `X-RCR-*` -> `X-Solvela-*` (legacy headers accepted).
- **Prometheus metrics**: `rcr_*` -> `solvela_*` prefix.
- **Dashboard**: All UI text updated from RustyClawRouter to Solvela.
- **Infrastructure**: Fly.io app, Docker, fly.toml updated to `solvela-gateway`.

## 2026-04-08 — Escrow Program Deployed to Mainnet

- **Escrow program deployed to Solana mainnet**: Program ID `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`, upgrade authority retained by deployer. Built with Anchor 0.31.1, deployed via `solana program deploy`.
- **Gateway advertises escrow scheme**: 402 responses now include both "exact" and "escrow" payment schemes. Escrow program ID served from Fly.io secret.
- **Program ID updated across entire codebase**: All source files, tests, docs, and config updated from placeholder to mainnet program ID.
- **Regulatory docs updated**: `regulatory-position.md` updated to reflect mainnet deployment with upgrade authority retained.

## 2026-04-07 — First Real Payment + Production Fixes + Telsi Migration Complete

- **Telsi.ai migration to Solvela complete**: Telsi has successfully migrated from BlockRun to Solvela. Second production product now live on the gateway, processing real payments.
- **First real USDC payment processed**: Telsi Telegram app sent real USDC payment through Solvela on Solana mainnet, received LLM response. End-to-end payment flow verified with actual money on mainnet.
- **Critical Fly.io config fix (PR #5)**: Gateway was calling `AppConfig::default()` and ignoring all Fly.io env vars for Solana configuration. Root cause: missing `config/default.toml` load in startup path. Result: `recipient_wallet` was always empty despite env vars being set. Fixed by loading `config/default.toml` first, then applying env var overrides. Deployed to production.
- **CLI resource URL fix (PR #6)**: CLI was sending full URL in payment resource field. Gateway validates resource as path only (per x402 spec). One-line fix: send path instead of full URL.
- **PR #4 follow-up fixes**: Python SDK error hardening (ImportError fails with key message, session_spent set after success only, specific exception catches). CLI hardening (non-zero exit on errors, panic-safe env cleanup, RPC error handling, empty response warnings).
- **Transaction format compatibility verified**: Architect confirmed gateway accepts both legacy transactions (CLI) and v0 versioned transactions (Python/TypeScript SDKs) via `ParsedMessage::from_bytes()` version prefix detection.
- **Known issue**: Second test request hit 429 rate limit from LLM provider. Payment was processed but no response returned. This is exact use case for escrow (pay only for what you receive).
- **6 PRs merged total** (#1-6 all merged). Status: Gateway deployed and processing real payments on mainnet.

## 2026-04-06 — Test Coverage, Real Signing, Product Docs, Error Hardening

- **CLI test suite**: 0 → 30 tests across all 8 commands (wallet, chat, models, health, stats, doctor, nonce, services). wiremock for HTTP mocking, tempfile for filesystem isolation, test isolation, error cases.
- **Real Solana signing**: Replaced STUB_BASE64_TX in Python SDK (solders + spl-token for signing) and CLI (x402 crate types, no new deps). TypeScript SDK already had signing. MCP server kept stub intentional.
- **Product documentation**: Added to `docs/product/` — regulatory-position.md (attorney-ready), how-it-works.md, use-cases.md, faq.md.
- **Error hardening**: Python SDK ImportError hard-fails with key, session_spent after success only, specific exception catches. CLI: non-zero exit on errors, empty response warnings.
- **PR review + fixes**: Comprehensive 5-agent review of enterprise + A2A features (2026-04-05 PR #1). All 5 critical + 8 important + 10 suggestions fixed: privilege escalation guard, fail-closed budgets, API key debug redaction, audit actor fields, type safety, 26 validation tests added.
- **Doc restructure**: Split into CLAUDE.md (how to work), HANDOFF.md (current state), CHANGELOG.md (history). Removed hardcoded test counts from CLAUDE.md.
- **Gateway deployed** to Fly.io with all enterprise + A2A features live (solvela-gateway.fly.dev).
- **Escrow program status** verified: NOT deployed to any network. Program ID is local testing only.

## 2026-04-05 — A2A Protocol Adapter + Enterprise Polish

- **A2A protocol adapter**: `GET /.well-known/agent.json` (AgentCard discovery), `POST /a2a` (JSON-RPC 2.0 dispatcher), `message/send` handler with full x402 payment flow, Redis-backed task state store. 7 commits, 38 new tests.
- **Enterprise follow-up**: `OrgRole` enum replaces `role: String`, split `routes/orgs.rs` (2026 lines) into 7 submodules, wired `RequireOrg`/`RequireOrgAdmin` extractors to all org endpoints, dashboard structured error handling (`ApiResult<T>`).
- Regulatory research: AP2 safe/unsafe boundary documented. Discovery + x402 settlement = safe. Fiat/card processing = requires licensing.

## 2026-04-04 — x402 V2 Migration

- Migrated from x402 V1 to V2 wire format.

## 2026-03-31 — Production Wiring

- Wired Terminal → RCR (removed direct Anthropic key from rclawterm-gateway)
- Set all 5 provider API keys on Fly.io (Anthropic, xAI, DeepSeek added)
- Funded fee-payer wallet (0.09 SOL)
- Redeployed gateway with Dockerfile fix (`crates/common/` → `crates/protocol/`)
- LiteSVM integration tests: 14 tests for escrow program (5 happy path + 9 error)
- Installed Anchor CLI 0.31.1 + Solana toolchain 3.1.12

## 2026-03-29 — Dashboard API Integration

- All dashboard pages fetch real data from gateway API
- Admin aggregate stats endpoint (`GET /v1/admin/stats`)
- Graceful mock-data fallback with warning banner
- Admin auth via `GATEWAY_ADMIN_KEY` env var (server-side only)
- 82 dashboard tests passing

## 2026-03-18 — Terminal Backend Deploy

- `rclawterm-gateway.fly.dev` deployed, 2 machines (ord)
- Shared Upstash Redis instance (`solvela-cache`)
- OpenAI + Google API keys set on Fly.io

## Earlier — Core Gateway (Phases 1-4, 8-9, 12-14)

- **Phases 1-3**: Axum HTTP server, x402 middleware, 5 LLM providers, 15-dimension smart router, 4 routing profiles, Redis cache, circuit breaker, Python/TS/Go/MCP SDKs, CLI
- **Phase 4**: Anchor escrow program (deposit/claim/refund, PDA vault, timeout refunds)
- **Phase 8**: Escrow hardening (claim queue, claim processor, fee payer pool rotation, durable nonces, circuit breaker, exponential backoff, escrow metrics)
- **Phase 9**: Service marketplace (external service proxy, admin registration, health checker)
- **Phase 12**: Prometheus monitoring (15 metrics, `/metrics` endpoint)
- **Phase 13**: Documentation overhaul
- **Phase 14**: Production hardening (CatchPanicLayer, timeouts, connection limits, graceful shutdown)
- **Gateway extras**: Debug headers, stats endpoint, session tracking, SSE heartbeat, nonce endpoint
- **Security audits**: Multiple rounds — 7 CRITICAL, 7 HIGH, 4 HIGH, 12 MEDIUM — all resolved
- **Chat route refactor**: Monolithic `chat.rs` (2405 lines) → `chat/` module directory
