# Solvela -- Regulatory Position

> **Purpose**: Technical description of how money flows in the Solvela system, prepared for attorney consultation. This document describes system behavior, not legal conclusions.

> **Date**: May 2026 (last refreshed 2026-05-04 — substantive audit, not date-only)

---

## What Solvela Is

Solvela (RCR) is **software infrastructure** -- a protocol adapter and API proxy written in Rust. It sits between AI agents and LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek) and does three things:

1. **Routes HTTP requests** from agents to the appropriate LLM provider.
2. **Verifies on-chain payment signatures** on the Solana blockchain before proxying requests.
3. **Returns LLM responses** to the requesting agent.

RCR is comparable to a reverse proxy (like Nginx or Cloudflare) that checks for a valid payment receipt before forwarding a request. The payment itself happens on the Solana blockchain, not through RCR.

## What Solvela Is Not

- **Not a money transmitter.** RCR does not move, hold, or control funds. It reads blockchain state to verify that a transfer occurred.
- **Not a custodian.** RCR never has access to private keys or the ability to move funds on behalf of users.
- **Not a currency exchange.** RCR does not convert between currencies, fiat or crypto.
- **Not a payment processor** in the traditional sense (Stripe, Square). It does not initiate, settle, or reverse transactions. It verifies that a transaction already happened on-chain.

## How Money Flows

### Direct Payment (scheme: "exact")

```
Agent Wallet ──[USDC transfer on Solana]──> Recipient Wallet (gateway operator)
                        │
                        │  (Solana blockchain settles this transfer)
                        │
                  RCR software reads the transaction signature
                  and verifies it matches the expected amount,
                  recipient, and token mint.
```

- The agent constructs and signs a Solana transaction that transfers USDC-SPL from its wallet to the gateway operator's wallet.
- The agent sends the signed transaction (or its signature) to RCR in an HTTP header.
- RCR calls Solana RPC to verify the transaction: correct amount, correct recipient, correct token, not a replay.
- **RCR never touches the funds.** The transfer is wallet-to-wallet on Solana. RCR is a read-only observer of the blockchain.

### Escrow Payment (scheme: "escrow")

```
Agent Wallet ──[deposit USDC]──> PDA Escrow Account (on-chain program)
                                        │
                              ┌─────────┴──────────┐
                              │                     │
                     After service delivery:   If unclaimed after timeout:
                     Gateway claims actual     Agent reclaims full deposit
                     cost; remainder refunds   (no gateway involvement needed)
                     to agent automatically.
```

- The agent deposits USDC into a **Program Derived Address (PDA)** -- an account controlled by an Anchor smart contract deployed on Solana, not by any person or server.
- The PDA address is deterministically derived from the agent's public key and a service identifier. No one holds a private key to this account.
- The smart contract has three instructions:
  - **Deposit**: Agent locks funds with a maximum amount and an expiry slot.
  - **Claim**: Gateway operator claims the actual cost (must be less than or equal to deposited amount). The remainder is automatically transferred back to the agent's token account in the same transaction.
  - **Refund**: After the expiry slot, the agent can reclaim the full deposit. This requires no cooperation from the gateway.
- The smart contract logic is deterministic and verifiable on-chain. The gateway software submits a claim transaction, but the on-chain program enforces the rules (amount limits, expiry, refund eligibility).

### Revenue

RCR charges a **5% platform fee** on every request. This fee is included in the payment amount the agent is asked to pay. The payment goes to the gateway operator's wallet. There is no separate fee collection mechanism -- the fee is simply part of the price.

### Optional Gas Sponsorship (Fee Payer)

RCR includes an optional mode in which the gateway acts as the Solana **fee payer** for the agent's transaction -- meaning the gateway pays the small SOL network fee (typically ~5,000 lamports / ~$0.0005) so that an agent without SOL in its wallet can still transact in USDC. This is configured per deployment and is off by default.

- The fee payer signature **only authorizes paying the network's transaction fee**. It does not authorize any token transfer, account creation outside of the fee payer's own SOL deduction, or movement of agent funds.
- The gateway does not receive USDC by acting as fee payer; it pays a tiny amount of SOL out of its own operator wallet on the agent's behalf as a UX convenience.
- This is functionally identical to the gas-sponsorship pattern used by major Solana wallets and dApps (e.g., Phantom session-key flows, Squads multisig relayers).
- **Why it does not change the regulatory analysis**: subsidizing a counterparty's network fee is not a transmission of value to or from a third party. The agent's USDC still moves agent-to-recipient. The gateway's SOL still moves operator-to-validators.

## Key Regulatory Distinctions

### No Custody of Funds

- The gateway software **never holds private keys** to any wallet other than its own operator wallet.
- In the direct payment flow, funds move wallet-to-wallet on Solana. RCR verifies the transaction signature (a read operation) but cannot initiate, reverse, or redirect the transfer.
- In the escrow flow, funds are held by a PDA controlled by on-chain program logic. The gateway submits claim transactions, but the on-chain program validates and executes them. The gateway cannot withdraw more than the actual cost, and the agent can always reclaim unclaimed funds after timeout.

### No Fiat Currency Involvement

- RCR operates exclusively in **USDC-SPL** (a dollar-pegged stablecoin) on the **Solana blockchain**.
- There is no fiat on-ramp or off-ramp. Agents must already hold USDC-SPL in a Solana wallet before using RCR.
- RCR does not accept credit cards, bank transfers, ACH, wire transfers, or any fiat payment method.

### No Account Creation or Identity Collection

- Agents are identified solely by their Solana wallet address (a public key).
- RCR does not collect names, emails, phone numbers, or any personally identifiable information.
- There is no sign-up process, no KYC/AML collection, and no user database in the traditional sense.
- Enterprise features (teams, API keys) are opt-in organizational tools -- they do not involve identity verification.

**Concretely, the org/team data model stores only**: an organization display label, an organization slug, the owner's Solana wallet address, member wallet addresses with role (owner/admin/member), and API key metadata (a non-secret prefix, a human-supplied label, role, optional expiry, last-used and revoked timestamps). API key secrets are stored as hashes; the plaintext is shown once at creation and redacted in logs and debug output. No email, no phone, no real name, no government identifier, no payment instrument. The org/team tables themselves do not store IP addresses; see **IP address handling** below for the narrow places where IP is collected.

### IP address handling

IP addresses are touched in exactly two narrow places, and never as a billing or identity link to LLM traffic:

1. **Transient rate-limit buckets** (`crates/gateway/src/middleware/rate_limit.rs`). Per-IP request counters held in memory or Redis to throttle abusive unidentified clients. These expire on the rate-limit window and are never persisted to PostgreSQL or written to long-term logs.
2. **Admin-action audit trail** (`audit_logs.ip_address`, migration `006_audit_logs.sql`). Records the originating IP for **administrative actions only** — e.g., "this wallet created an organization", "this API key was revoked". This is a SOC-2-style accountability trail expected by any acquirer's diligence; it does not record LLM request traffic, prompts, responses, or payment events.

Audit-log IPs are stored against the actor wallet (a public key) for security review. They are not joined to billing rows, not exposed publicly, and can be deleted by row-level wipe with no third-party coordination.

### Prompt and response content

The gateway **does not persist any LLM request or response content** — neither to PostgreSQL, nor to Redis, nor to long-term logs. There is no `prompt`, `message`, `content`, `body`, or equivalent column anywhere in the schema. The prompt-injection / jailbreak / PII-detection middleware (`crates/gateway/src/middleware/prompt_guard.rs`) is pattern-based, runs in-memory only, has no `tracing::` or `log::` calls that emit message text, and emits a boolean classification — not the matched content. Operators wishing to add request/response logging would need to add a code path that does not exist today; this is intentional.

### Data Persistence and Storage

For acquirer due diligence, the persisted data surface is small and intentionally non-personal:

| Datastore | What is stored | What is not stored |
|---|---|---|
| PostgreSQL — org/team tables (`organizations`, `teams`, `org_members`, `team_wallets`, `api_keys`) | Display labels, slugs, member wallet addresses with role, API key metadata (hash, prefix, label, role, last-used/revoked/expiry timestamps) | Email, phone, real name, government identifier, payment card data, plaintext API keys |
| PostgreSQL — `spend_logs` (per-request usage log) | One row per completed LLM request: wallet address, model name, provider name, input-token count, output-token count, USDC cost, Solana transaction signature, optional opaque request and session correlation IDs (`request_id`, `session_id`), timestamp | **Not** prompt text, **not** response text, **not** request body, **not** response body. Token *counts* are stored; token *content* is never stored. |
| PostgreSQL — billing tables (`wallet_budgets`, `team_budgets`) | Per-wallet and per-team spend caps and aggregated counters denominated in USDC (hourly / daily / monthly) | Card data, bank data, fiat balances |
| PostgreSQL — `audit_logs` (admin-action trail) | Action name, resource type, actor wallet, actor API key id, optional `details` JSONB describing the admin action, originating IP, timestamp | Prompts, responses, request/response bodies, LLM traffic of any kind |
| PostgreSQL — `escrow_claim_queue` (claim-job durability) | Pending escrow-claim transaction parameters (deposit pubkey, amount, retry counters) | Wallet keys, prompt content |
| Redis (Upstash) | Transient rate-limit counters keyed by IP or wallet, x402 nonce-replay window | Long-term user data, prompts, responses |
| Solana RPC (read-only) | No persistence; RCR is a client | — |

There is no data-subject-access workflow because there is no data subject under GDPR/CCPA -- a Solana public key is not personal data on its own and RCR does not link it to identity attributes. Wallet usage history can be wiped by deleting the corresponding rows; this requires no third-party coordination.

### On-Chain Settlement Only

- All payment settlement occurs on the Solana blockchain.
- Transaction finality is determined by Solana consensus, not by RCR.
- RCR uses Solana RPC calls to read transaction status. It does not run a validator node or participate in consensus.

## Regulations to Discuss

### FinCEN Money Services Business (MSB) Registration

**Question**: Does verifying payment signatures and operating a gateway constitute "money transmission" under the Bank Secrecy Act?

**Relevant facts**:
- RCR verifies that a Solana transaction has occurred. It does not initiate, execute, or settle the transaction.
- In the direct payment flow, RCR is a read-only observer. The agent and the blockchain handle the transfer.
- In the escrow flow, the on-chain program (not RCR) controls fund custody and settlement logic.
- RCR does charge a 5% fee, but this fee is collected as part of the payment to the operator's wallet -- there is no separate money movement.

### State Money Transmitter Licenses

**Question**: Do individual state definitions of "money transmission" capture RCR's signature-verification role?

**Relevant facts**:
- 49 states have varying definitions of money transmission.
- Some states define transmission broadly (any involvement in transferring value); others focus on receiving and transmitting funds.
- RCR does not receive funds in transit. In direct payment, funds go from agent wallet to operator wallet. In escrow, funds go from agent wallet to a PDA.

### Escrow PDA -- Custodial Wallet Considerations

**Question**: Could the PDA escrow account be classified as a "custodial wallet" by regulators?

**Relevant facts**:
- The escrow program has been **deployed to Solana mainnet** (program ID: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`, tx: `XGtZf6KHWnis6bY8T8NCULsngdC2kqy3GkuppVriyFFqJ8ud2NiBkAgcBRsYjvUMmMZcLUmYBw9RhhUnNNRYnZx`) with a 20-test suite (14 LiteSVM integration tests in `programs/escrow/tests/integration.rs` + 6 unit tests in `programs/escrow/tests/unit.rs`).
- The PDA is controlled by the on-chain program logic, not by the gateway operator. No one holds a private key to the PDA.
- The program logic enforces: claim amounts cannot exceed deposited amount, claims must occur before expiry slot, agent can unilaterally reclaim after timeout.
- **Upgrade authority is currently retained** by the deployer (`B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`). This allows bug fixes but regulators could argue de facto control. Authority can be revoked at any time via `solana program set-upgrade-authority --final` to make the program immutable. **Pre-acquisition action item**: verify current authority on-chain (`solana program show 9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`) and decide whether to finalize before any due diligence cycle.
- FinCEN guidance on smart-contract-controlled wallets is evolving and has not definitively addressed this pattern.

### California Digital Financial Assets Law (DFAL)

**Question**: Does DFAL, effective July 2026, create new obligations for RCR?

**Relevant facts**:
- DFAL regulates "digital financial asset business activity" including exchanging, transferring, or storing digital financial assets.
- RCR does not exchange or store digital assets. The question is whether signature verification constitutes "transferring."
- DFAL includes exemptions for software providers and technology platforms that do not control customer funds.
- **Action item**: As of this document's date (May 2026), DFAL goes live in roughly 8 weeks. Schedule attorney review before July 2026 to confirm the software-provider exemption applies and whether any disclosure or registration is required if California users are routed.

### SEC Considerations

**Relevant facts**:
- USDC is a stablecoin issued by Circle and is generally not classified as a security.
- RCR does not issue, sell, or create any tokens or digital assets.
- RCR does not operate an exchange or trading platform: it does not match buyers and sellers, does not custody assets between counterparties, does not set prices for third-party services, and does not take a financial position in any service it routes to.
- The escrow program facilitates payment for services, not investment or speculation.

### x402 Service Registry

RCR exposes a `GET /v1/services` endpoint that returns a directory of x402-compatible services that have opted into discovery through the gateway. This is a routing aid, not a marketplace in the regulated sense:

- **What it is**: a directory of HTTP endpoints (each provider's own URL, x402 metadata, and a per-request price set by that provider). Comparable to a DNS record or an OpenAPI catalog. Data is loaded from `config/services.toml` and an admin-only runtime registration API.
- **What it is not**: RCR does not custody payments to third-party services, does not guarantee delivery, does not handle disputes, does not pool fees across providers, and does not enable provider-to-provider trading.
- **Current production state**: the registry ships with no third-party services enabled (`config/services.toml` is committed with all entries commented out under a `Phase 6 is NOT YET IMPLEMENTED` header). Until a third-party provider is registered, the directory is empty in production.
- **When third-party services are added**: payments to those services follow the same x402 verification flow described above — agent wallet to provider wallet on Solana, with RCR as a read-only verifier of the payment signature. RCR does not sit in the funds path.

## What We Deliberately Do Not Do

These boundaries are architectural decisions made to avoid regulatory exposure:

| Excluded capability | Why it matters |
|---|---|
| No fiat currency processing | Avoids money transmitter classification under most state laws |
| No card payments | No PCI compliance, no payment processor registration |
| No fiat-to-crypto conversion | No exchange registration, no FinCEN MSB for exchange activity |
| No custodial fund holding | Gateway never controls funds; PDA is program-controlled |
| No user accounts or KYC | No data privacy obligations (CCPA, GDPR) from identity collection |
| No AP2 mandate signing (JWS/SD-JWT) | Avoids acting as a payment facilitator under card network rules |

### A2A and AP2 Compatibility

RCR implements compatibility with Google's **Agent-to-Agent (A2A)** protocol and the **Agentic Payments (AP2)** specification, but only the parts that do not involve fiat:

- **Implemented**: Agent discovery via `AgentCard` (`.well-known/agent.json`), x402 payment settlement flow within the A2A `message/send` lifecycle.
- **Explicitly excluded**: The AP2 specification includes a Managed Payment Provider (MPP) flow for card-based payments. Implementing MPP would require acting as a payment facilitator, which triggers MSB registration and state licensing. We do not implement MPP.

## Summary of Money Flow

| Flow | Who moves the money | Who holds the money | RCR's role |
|---|---|---|---|
| Direct payment | Agent (signs tx) + Solana (settles) | Agent wallet, then operator wallet | Verifies tx signature (read-only) |
| Escrow deposit | Agent (signs tx) + Solana (settles) | PDA (program-controlled) | Verifies deposit tx signature |
| Escrow claim | Gateway (signs tx) + Solana (settles) | PDA, then split to operator + agent | Submits claim tx; on-chain program enforces rules |
| Escrow refund | Agent (signs tx) + Solana (settles) | PDA, then agent wallet | Not involved; agent acts unilaterally |
| Optional fee-payer | Gateway co-signs tx for SOL fee only | Operator wallet pays a few thousand lamports of SOL to validators | Pays network fee for the agent; never authorizes USDC movement |

---

## Document Hygiene

This document describes system behavior at the date noted in the header. It is intended to be re-verified before any of the following:

- A grant application that references regulatory posture
- An attorney consultation
- A due-diligence cycle (acquisition, partnership, or audit)
- Any material change to: payment flow, providers, custody model, supported tokens, supported chains, fee structure, or persisted data fields

**Re-verification checklist** (run before any of the above):

1. Confirm `PLATFORM_FEE_PERCENT` in `crates/protocol/src/constants.rs` still matches the percentage stated here.
2. Confirm `USDC_MINT` constant still hardcoded to mainnet USDC; confirm verifier still rejects other mints.
3. Confirm provider list in `crates/gateway/src/providers/` matches the count and names referenced anywhere in this doc (currently: `openai.rs`, `anthropic.rs`, `google.rs`, `xai.rs`, `deepseek.rs` — five providers).
4. Confirm escrow program ID in `programs/escrow/src/lib.rs` (`declare_id!()`) matches the ID quoted here.
5. Confirm escrow test count by counting `fn test_` in `programs/escrow/tests/*.rs`.
6. Run `solana program show <program-id>` against mainnet and update the upgrade-authority status above (still retained vs finalized).
7. Confirm `config/services.toml` reflects the production state described in the x402 Service Registry section (currently: empty / Phase 6 not enabled).
8. Confirm no code under `crates/` references Stripe, card, fiat, ACH, wire, or AP2 Managed Payment Provider (MPP) flows. Use case-insensitive search; ignore `AGENTS.md` doc files which discuss what we deliberately do *not* implement.
9. Confirm org/team data model in `crates/gateway/src/orgs/models.rs` still excludes email, phone, real name, government ID, and payment instruments.
10. **Schema drift check**: list every `.sql` under `migrations/` and confirm the data-persistence table above accurately describes each table. Adding a new migration that introduces personal data (email, phone, IP, request body, response body, prompt content) requires updating this document before merge.
11. **IP persistence check**: grep `ip_address|client_ip|peer_addr|x-forwarded-for|x-real-ip` across `crates/gateway/src` and `migrations/`. The only acceptable hits are (a) `crates/gateway/src/middleware/rate_limit.rs` for transient throttling and (b) `audit_logs.ip_address` for admin-action telemetry. New hits in any other file invalidate this document.
12. **Prompt-content persistence check**: grep `prompt|message|content|body` across `migrations/*.sql`. There must be zero column matches for these names. Any match indicates a prompt-logging change that requires updating both this document and the privacy disclosures before it ships. (Note: `spend_logs` stores token *counts* — `input_tokens`, `output_tokens` — never token text. The grep covers the right names to catch the hazardous case.)
13. **Logging silence check**: grep `tracing::|log::|warn!|info!|error!|debug!` inside `crates/gateway/src/middleware/prompt_guard.rs`. Any new log line in that file must be reviewed to ensure it emits a classification, not the matched message text.

If any of these checks fail, this document must be revised before the trigger event proceeds.
