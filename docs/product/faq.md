# Frequently Asked Questions

## Is this legal?

We are actively consulting with legal counsel on the regulatory implications of the system. RustyClawRouter operates exclusively in USDC (a dollar-pegged stablecoin) on the Solana blockchain. It does not process fiat currency, hold custody of user funds, or perform currency exchange. We have deliberately excluded features -- card payments, fiat conversion, custodial wallets -- that would trigger money transmitter licensing requirements. The regulatory landscape for crypto payment infrastructure is evolving, and we are tracking it closely.

## Who controls the money?

The agent controls its own wallet. In a direct payment, USDC moves from the agent's wallet to the gateway operator's wallet on the Solana blockchain. RustyClawRouter verifies that the transfer happened but never touches the funds.

In an escrow payment, USDC is held by a smart contract on Solana -- a program with deterministic rules, not a person or company. The gateway can claim only the actual cost, and the agent can reclaim unclaimed funds after a timeout. Neither party needs to trust the other.

## What happens if the payment fails?

If the payment signature is invalid, the amount is wrong, or the transaction didn't settle on Solana, RCR rejects the request immediately with a clear error message. The agent is not charged. No partial charges, no pending states. Either the payment is verified and the request proceeds, or it doesn't.

## What happens if the LLM provider is down after payment?

If the payment is verified but the LLM provider fails, the agent has already paid. In the direct payment flow, the payment has settled on Solana and cannot be reversed -- this is a known tradeoff of blockchain settlement.

For this reason, the **escrow flow** is recommended for high-value requests. With escrow, the funds are locked but not yet claimed. If the gateway cannot deliver the service, it does not submit a claim, and the agent reclaims the deposit after the timeout period.

## How fast is the payment?

Sub-second. Solana transaction confirmation typically takes 1-2 seconds. The full round-trip -- send request, receive price, sign payment, verify, proxy to provider, return response -- typically completes in 1-3 seconds, depending on the LLM provider's response time. The payment verification adds less than a second of overhead.

## What wallets work?

Any Solana wallet that can sign transactions and hold USDC-SPL tokens. For autonomous agents, this is typically a keypair generated programmatically. For human users testing the system, any Solana wallet (Phantom, Solflare, Backpack) works through the SDKs.

## Can I use this without Solana?

Not today. RustyClawRouter currently supports only Solana with USDC-SPL. The architecture is designed for multi-chain support (the payment verification system uses a chain-agnostic trait), and Base/EVM support is planned for the future. But right now, you need a Solana wallet with USDC.

## How much does it cost?

The cost per request depends on the model used and the number of tokens consumed. RCR adds a **5% platform fee** on top of the provider's cost. Every response includes a cost breakdown:

- **Provider cost**: What the LLM provider charges (e.g., $0.0025)
- **Platform fee**: 5% of the provider cost (e.g., $0.000125)
- **Total**: What the agent pays (e.g., $0.002625)

All prices are in USDC, which is pegged 1:1 to the US dollar. There are no monthly fees, no minimum charges, and no hidden costs.

## Is the code open source?

The core protocol library (`rustyclaw-protocol`) and SDKs (Python, TypeScript, Go) are available on GitHub. The full gateway source code is in the repository. Open-source publishing to crates.io, npm, and PyPI is planned.

## What's the escrow and why would I use it?

The escrow is a smart contract on Solana that holds funds in a neutral account until service delivery is confirmed.

**Use escrow when:**
- The final cost is uncertain (e.g., a long conversation where you don't know how many turns it will take).
- You're making a high-value request and want protection against gateway failure.
- You don't want to overpay -- the escrow refunds the difference between the deposit and the actual cost automatically.

**How it works:**
1. You deposit the maximum estimated cost into the escrow (a program-controlled account on Solana).
2. RCR processes your request.
3. The smart contract releases the actual cost to the gateway operator and refunds the rest to you.
4. If the gateway never claims (server crash, network issue), you reclaim everything after the timeout.

The escrow adds one extra transaction (the deposit) but gives you trustless guarantees that direct payment does not.
