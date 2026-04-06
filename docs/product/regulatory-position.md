# RustyClawRouter -- Regulatory Position

> **Purpose**: Technical description of how money flows in the RustyClawRouter system, prepared for attorney consultation. This document describes system behavior, not legal conclusions.

> **Date**: April 2026

---

## What RustyClawRouter Is

RustyClawRouter (RCR) is **software infrastructure** -- a protocol adapter and API proxy written in Rust. It sits between AI agents and LLM providers (OpenAI, Anthropic, Google, xAI, DeepSeek) and does three things:

1. **Routes HTTP requests** from agents to the appropriate LLM provider.
2. **Verifies on-chain payment signatures** on the Solana blockchain before proxying requests.
3. **Returns LLM responses** to the requesting agent.

RCR is comparable to a reverse proxy (like Nginx or Cloudflare) that checks for a valid payment receipt before forwarding a request. The payment itself happens on the Solana blockchain, not through RCR.

## What RustyClawRouter Is Not

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
- The escrow program has been **designed and tested locally** (21 tests passing) but **has not been deployed** to Solana mainnet or devnet. The program ID in the codebase is a locally-generated keypair for testing only.
- When deployed, the PDA will be controlled by the on-chain program logic, not by the gateway operator. No one holds a private key to the PDA.
- The program logic enforces: claim amounts cannot exceed deposited amount, claims must occur before expiry slot, agent can unilaterally reclaim after timeout.
- **Deployment decision pending attorney guidance**: Upon deployment, upgrade authority can be either **retained** (allows bug fixes, but regulators could argue de facto control) or **revoked** (program becomes immutable, strongest "no human control" argument). This decision should be made with legal counsel.
- FinCEN guidance on smart-contract-controlled wallets is evolving and has not definitively addressed this pattern.

### California Digital Financial Assets Law (DFAL)

**Question**: Does DFAL, effective July 2026, create new obligations for RCR?

**Relevant facts**:
- DFAL regulates "digital financial asset business activity" including exchanging, transferring, or storing digital financial assets.
- RCR does not exchange or store digital assets. The question is whether signature verification constitutes "transferring."
- DFAL includes exemptions for software providers and technology platforms that do not control customer funds.

### SEC Considerations

**Relevant facts**:
- USDC is a stablecoin issued by Circle and is generally not classified as a security.
- RCR does not issue, sell, or create any tokens or digital assets.
- RCR does not operate a marketplace, exchange, or trading platform.
- The escrow program facilitates payment for services, not investment or speculation.

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
