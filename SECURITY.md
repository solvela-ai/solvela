# Security

## Reporting a vulnerability

Email **security@solvela.ai** with details. We acknowledge within 1 hour for P0 (custody/payment/auth) and respond within 24 hours for everything else. Do not open public GitHub issues for security reports.

A `.well-known/security.txt` is served from `api.solvela.ai`.

## Current security posture

The gateway (`api.solvela.ai`) and dashboard (`solvela.ai`, `app.solvela.ai`, `docs.solvela.ai`) run on a clean dependency tree (`cargo audit` and `npm audit` both pass at HEAD).

The escrow program is deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Upgrade authority is held at `EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4` (single-sig, stored on the operator workstation, not air-gapped). Authority was migrated on 2026-05-08 from the gateway's hot-wallet keypair (`B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`) so a runtime compromise of the gateway can no longer escalate to upgrade rights over the deployed program. Migration to a Squads multisig (or hardware-wallet-backed authority) is the next planned step.

## Known transitive advisories — accepted with reason

A small set of advisories cannot be cleared without a major ecosystem migration. These are accepted for the reasons given below, tracked, and re-evaluated on every dependency review.

### Rust (gateway workspace)

| ID | Crate | Severity | Reason |
|---|---|---|---|
| RUSTSEC-2023-0071 | `rsa` 0.9.x | medium | No upstream fix exists. Transitive via `sqlx-mysql` only; Solvela uses `sqlx-postgres`, so the vulnerable code path is unreachable. Suppressed via `cargo audit --ignore RUSTSEC-2023-0071` in CI with rationale comment. |

### Rust (escrow program — `programs/escrow/Cargo.lock`)

The escrow program is built against the Solana 1.18 / Anchor 0.30 toolchain (matches the deployed mainnet bytecode). Solana 1.x pins a number of legacy transitive dependencies that cannot be upgraded individually:

| Crate | Advisory | Status |
|---|---|---|
| `curve25519-dalek` 3.2.0 | RUSTSEC-2024-0344 | Pinned by Solana 1.18 SDK. Resolves on Solana 2.x migration. |
| `ed25519-dalek` 1.0.1 | RUSTSEC-2022-0093 | Pinned by Solana 1.18 SDK. Resolves on Solana 2.x migration. |
| `rustls-webpki` 0.101.7 | RUSTSEC-2026-0098, 0099, 0104 | Pinned via legacy `rustls` major in Solana 1.x dep tree. |
| `bincode` 1.3.3 | RUSTSEC-2025-0141 | Pinned by Solana 1.x; bincode 2.x is a major-API break. |
| `rand` 0.7.3 | RUSTSEC-2026-0097 | Transitive via `solana-sdk` 1.x. |
| `libsecp256k1` 0.6.0 | RUSTSEC-2025-0161 | Anchor 0.30 transitive. |
| `rustls-pemfile` 1.0.4 | RUSTSEC-2025-0134 | Solana 1.x transitive. |
| `ansi_term`, `atty`, `derivative`, `paste` | various unmaintained | All Solana 1.x build-time transitives. |

**Risk evaluation:** The deployed program bytecode is fixed at deploy time and is not affected by these advisories at runtime — the vulnerable code paths live in build-time transitives, not in the on-chain bytecode. The 2026-05-08 redeploy (slot 418438627) used the same Anchor 0.31.1 / Solana 1.x toolchain, so the transitive-advisory profile is unchanged.

**Resolution path:** Solana 2.x / `@solana/kit` migration is the next planned escrow redeploy. Tracking issues [#155](https://github.com/solvela-ai/solvela/issues/155) and [#156](https://github.com/solvela-ai/solvela/issues/156); no fixed target date — the deployed program is stable on mainnet and the migration will be coordinated with the broader ecosystem move.

### npm (Solana web3.js 1.x ecosystem — `sdks/mcp`, `sdks/openclaw-provider`, `integrations/openclaw`)

| Package | Advisory | Status |
|---|---|---|
| `bigint-buffer` (transitive) | GHSA-3gc7-fjrx-p6mg (high — buffer overflow in `toBigIntLE()`) | No upstream fix. Pulled in via `@solana/spl-token` → `@solana/buffer-layout-utils`. |
| `uuid` <14.0.0 (transitive) | GHSA-w5hq-g745-h8pq (moderate — buffer bounds) | Pulled in via `rpc-websockets` → `@solana/web3.js` 1.x → `jayson`. |

`npm audit fix --force` resolves these by *downgrading* `@solana/spl-token` to 0.1.8 and `@solana/web3.js` to 0.9.2 — versions that pre-date the entire Solana program model used today. That is a regression, not a fix.

**Risk evaluation:** `bigint-buffer.toBigIntLE()` is called inside `@solana/spl-token` parsing routines. In our usage, inputs come from RPC responses for our own program account data — never untrusted user payloads. The exploitability surface is the RPC provider, which is Helius (signed traffic). This is documented but considered low real-world risk.

**Resolution path:** Migrate consuming SDKs from `@solana/web3.js` 1.x → `@solana/kit` (the v2 client). This is the same migration that resolves the Rust 1.x advisories above. Tracked, no fixed date.

## Dependency review cadence

- `cargo audit` and `npm audit` run on every PR via GitHub Actions
- Dependabot opens PRs for non-major bumps; major bumps reviewed manually
- Quarterly review of accepted-and-documented transitive advisories

## Pre-launch security reviews

Solvela's posture is to surface and patch latent flaws before they harm users, rather than wait for an external incident to drive remediation. Reviews surface findings, findings get tracked, fixes ship through normal PR flow, and the result is documented here for transparency.

### 2026-05-09 — Whole-repo security review pass

Internal review-pass across all 6 workspace crates plus the standalone Anchor escrow program. 18 must-ship PRs ([#182](https://github.com/solvela-ai/solvela/pull/182)–[#199](https://github.com/solvela-ai/solvela/pull/199)) landed on `main` and one cleanup-tier PR ([#200](https://github.com/solvela-ai/solvela/pull/200)) followed with 8 deferred MEDIUMs. Each of the 7 crate-level reviews was conducted by a fresh agent against the post-merge `main`, with findings triaged into CRITICAL/HIGH (must-ship) and MEDIUM (cleanup-tier) by severity bar. Mainnet escrow bytecode redeployed 2026-05-10 with the in-source fixes (see Redeployed section below).

#### Headline CRITICAL findings (deployed gateway, fixed at HEAD)

| Surface | Footgun | Fix | PR |
|---|---|---|---|
| Gateway proxy/services/config | 4 SSRF paths (DNS rebinding, CGNAT-mapped IPv4, redirect-to-internal, default-port heuristic) | Centralised SSRF guard with DNS pinning, IPv4-mapped-IPv6 exclusion, deny-by-default redirect policy | [#189](https://github.com/solvela-ai/solvela/pull/189) |
| Gateway balance monitor | `Client::build` error was swallowed; if it failed, the gateway started without monitoring and silently never alerted on low fee-payer balance | Propagate the error through `anyhow::Context`; gateway fails to start if monitor can't be built | [#194](https://github.com/solvela-ai/solvela/pull/194) |
| x402 verifier | Sysvar typo + missing provider check let a malformed deposit verify successfully | Correct sysvar pubkey + `has_one = provider` constraint enforced by the verifier | [#182](https://github.com/solvela-ai/solvela/pull/182) |

#### Headline CRITICAL findings (escrow program, redeployed to mainnet 2026-05-10)

These fixes are now **live in the deployed mainnet bytecode** (`9neDH…HLU`, slot 418832804, bytecode SHA-256 `5fb8e7548f0653a165803523c322b8d19eab06310462d8621c7250e912bf16c9`). Between the 2026-05-09 source merge and the 2026-05-10 redeploy, `getProgramAccounts` against the program ID returned zero accounts — the deployed bytecode handled no real customer escrows during the in-source-only window, so neither footgun had any opportunity to fire. The findings are documented under the disclosure-transparency principle, not because of any incident.

| Footgun | What could happen | Fix |
|---|---|---|
| **Vault dust stuffing** | `spl-token`'s `CloseAccount` aborts on any non-zero balance. Vault ATA address is publicly derivable and SPL transfers don't require recipient signature — anyone (including the agent) could transfer 1 dust unit into the vault between deposit confirmation and the gateway's claim. The claim's `close_account` CPI then fails, the entire claim tx reverts, the provider can never claim. After expiry the agent refunds full balance + dust at zero net cost. Net: agent gets the LLM response (already delivered HTTP-side before claim), provider gets nothing. | `claim` instruction now reloads the vault after the contracted transfers and drains any surplus to the agent's ATA before `close_account`. Tested via a regression test that injects 1 dust unit into the vault post-deposit and asserts `claim` succeeds. ([#198](https://github.com/solvela-ai/solvela/pull/198)) |
| **Instant-refund window via tight `expiry_slot`** | `expiry_slot > now` was the only lower bound on deposit. Agent could deposit with `expiry_slot = now + 1`, gateway verifies + delivers the LLM response after settlement, gateway's claim transaction can't realistically land before `slot >= expiry_slot`, claim fails with `EscrowExpired`, agent calls `refund` and recovers the deposit. Same blast radius as the dust attack via different mechanism. | Two-layer fix: new `MIN_EXPIRY_BUFFER = 50` slots (~20 s) on-chain in `deposit.rs`; off-chain verifier in `crates/x402/src/escrow/verifier.rs` now parses the previously-skipped `expiry_slot` field from the deposit instruction data, fetches current slot via a new `solana_rpc::get_current_slot` helper, and rejects deposits where `expiry_slot - current_slot < 50`. ([#198](https://github.com/solvela-ai/solvela/pull/198)) |

#### HIGH findings — closed at HEAD

Twenty-plus HIGH findings across the gateway (info-leak cluster, financial correctness, budget enforcement, multi-tenant authz role-cap), x402 (atomic claim queue, fee-payer counter race, nonce-pool authority check, secret-decode-buffer zeroing), router (load-time pricing validation), protocol (`#[serde(deny_unknown_fields)]` on every payment payload type, `Role::Unknown` forward-compat, `Usage::new` saturating constructor), CLI (`fs::write`+chmod race on the wallet file, missing payment-bearing HTTP timeouts, `expiry_slot` arithmetic wrappable to a near-zero value, global `--api-url` accepting `file://`, disk-loaded wallet address re-validation, `WalletData` redacted-Debug, `Zeroizing<>` on derived secret arrays). Per-PR breakdown in `CHANGELOG.md`.

#### Cleanup-tier wave 1 — 8 MEDIUMs shipped in PR #200

Defence-in-depth and quality items: HTTP timeouts on read-only CLI listings, removal of stray `.expect()` in production paths, surfacing previously-swallowed RPC errors via `tracing::warn!`, clearer `error_msg` paths, `Transfer` → `TransferChecked` migration across all 5 escrow token-transfer sites (defense-in-depth over existing mint pinning), and a deployment checklist for `--features mainnet` plus a `cargo build-sbf` fallback in `programs/escrow/AGENTS.md`. None security-critical.

#### Verification

- 18 must-ship PRs squash-merged to `main` in number order; bot approval (`solvela-per`) gated each merge to satisfy branch protection. PR #199 was rebased + force-pushed once to resolve a `mcp/mod.rs` conflict with #197's `validate_wallet` wrap.
- Per-crate test counts at HEAD: cli 189, x402 132, gateway 497, router 19, protocol 18; escrow 23 integration + 8 unit tests pass via `cargo build-sbf && cargo test --features sbf`.
- `cargo fmt --all -- --check` and `cargo clippy --workspace --all-targets --all-features -- -D warnings` both clean.

#### Redeployed

The escrow source fixes from #198 + #200 shipped to mainnet on 2026-05-10. Build was reproduced from `main` at commit `53d1c610` with `cargo build-sbf` (Anchor 0.31.1 toolchain), 23 LiteSVM integration tests + 8 unit tests passing, and bytecode SHA-256 verified post-deploy via `solana program dump` against the local artifact: `5fb8e7548f0653a165803523c322b8d19eab06310462d8621c7250e912bf16c9` (310,360 bytes). Redeploy tx: [`58ud2vrh8KsiwST8AsA9qUE7kJpN1NTDvaz8TfzqbG9RRkHM64tUMD974efAigzktVPg5DWXrbE6GWQd6wjFNkxb`](https://solscan.io/tx/58ud2vrh8KsiwST8AsA9qUE7kJpN1NTDvaz8TfzqbG9RRkHM64tUMD974efAigzktVPg5DWXrbE6GWQd6wjFNkxb), slot 418832804. Authority unchanged (`EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4`); fee paid by gateway hot wallet `B7reP7…` (~0.00001 SOL). `getProgramAccounts` returned zero at redeploy time; the upgrade was zero-impact.

### 2026-05-08 — Escrow program code review

Internal review of `programs/escrow/` surfaced two latent settlement-path footguns. Both were patched proactively and the bytecode redeployed to mainnet on the same day.

| Footgun | What could have happened | Fix |
|---|---|---|
| **Claim-vs-refund race at expiry boundary** | Once `slot >= expiry_slot`, both `claim` and `refund` were simultaneously valid. An adversarial provider could race the agent at the deadline and capture the deposit. | New guard requires `Clock::slot < expiry_slot` for `claim`. |
| **Refund deadlock on closed agent ATA** | An agent who closed their USDC ATA after deposit (a normal post-deposit move that reclaims rent) would find both `refund` and `claim`'s refund leg failing. The funds would have been permanently stuck in the vault PDA. | Both instructions now use `init_if_needed` so the agent's ATA is recreated on the fly when needed. |

**Exposure:** None observed. `getProgramAccounts` against the program ID returned zero accounts at the time of redeployment — the deployed bytecode had handled no real customer escrows since launch, so neither footgun had any opportunity to fire. The findings are documented here under the disclosure-transparency principle, not because of any incident.

**Source:** PR [#160](https://github.com/solvela-ai/solvela/pull/160) (8 source changes + 7 new tests). Redeploy ceremony tracked and closed in issue [#161](https://github.com/solvela-ai/solvela/issues/161). Bytecode hash `2293edfce3a0e7b1b0332bc32d9f7a9e58105a2a3f825ee05d8a4f7fbf57bf26`. Six secondary hardening items (USDC mint feature gate, provider self-key rejection, `expiry_slot` ceiling, `agent` constraint relaxation for PDA agents, `ClaimEvent.deposited` field, `vault > 0` refund guard, `checked_sub` for refund math) shipped alongside.

### 2026-05-08 — Upgrade-authority / fee-payer key separation

Subsequent operational hardening on the same day. The single keypair `B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A` was serving as **both** the gateway's fee-payer hot wallet (decryptable from the Fly.io secret store on every gateway boot) **and** the program's upgrade authority. A runtime compromise of the gateway would therefore have escalated directly to upgrade rights over the deployed program — an attacker could have replaced the bytecode with a version that drains the next deposit, transferred the upgrade authority away, etc.

**Fix:** generated a new offline single-sig keypair (`EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4`), seed phrase backed up out of band, and ran `solana program set-upgrade-authority` to transfer authority off the hot-wallet keypair. The fee-payer key remains `B7reP7rzz…` (no operational change to the gateway), but it no longer carries any upgrade power.

**What's still imperfect (tracked):** the new authority key is single-sig and stored on disk. The next planned step is migration to a [Squads](https://app.squads.so) multisig — the standard for Solana protocol upgrade authorities (Jupiter, Drift, Marinade, Phoenix, Pyth, etc.). Likely path: 1-of-1 Squads now (free, ~5 min), upgraded to 2-of-N with a hardware-wallet co-signer when budget and operational complexity allow.
