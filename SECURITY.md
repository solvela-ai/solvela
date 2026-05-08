# Security

## Reporting a vulnerability

Email **security@solvela.ai** with details. We acknowledge within 1 hour for P0 (custody/payment/auth) and respond within 24 hours for everything else. Do not open public GitHub issues for security reports.

A `.well-known/security.txt` is served from `api.solvela.ai`.

## Current security posture

The gateway (`api.solvela.ai`) and dashboard (`solvela.ai`, `app.solvela.ai`, `docs.solvela.ai`) run on a clean dependency tree (`cargo audit` and `npm audit` both pass at HEAD).

The escrow program is deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Upgrade authority is held at `EDM9pao5miQdJYfzCtZii9cVn5ZHTBJq84Y9yyqZbsr4` (single-sig, stored offline). Authority was migrated on 2026-05-08 from the gateway's hot-wallet keypair (`B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`) so a runtime compromise of the gateway can no longer escalate to upgrade rights over the deployed program. Migration to a Squads multisig (or hardware-wallet-backed authority) is the next planned step.

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
