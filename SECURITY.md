# Security

## Reporting a vulnerability

Email **security@solvela.ai** with details. We acknowledge within 1 hour for P0 (custody/payment/auth) and respond within 24 hours for everything else. Do not open public GitHub issues for security reports.

A `.well-known/security.txt` is served from `api.solvela.ai`.

## Current security posture

The gateway (`api.solvela.ai`) and dashboard (`solvela.ai`, `app.solvela.ai`, `docs.solvela.ai`) run on a clean dependency tree (`cargo audit` and `npm audit` both pass at HEAD).

The escrow program is deployed to Solana mainnet at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Upgrade authority is retained at `B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A` (single-sig). Migration to a Squads multisig is on the go-live playbook.

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

**Risk evaluation:** The deployed program bytecode is fixed at deploy time and is not affected by these advisories. The advisories matter only on a future *redeploy*, which is gated on Anchor upgrade-authority approval anyway.

**Resolution path:** Solana 2.x / Anchor 0.31 migration. Tracked in `docs/plans/` with no current target date — the program is stable on mainnet and a redeploy is not currently planned.

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
