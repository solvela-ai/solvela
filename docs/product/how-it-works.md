# How RustyClawRouter Works

## Why Now: The API Pricing Shift

LLM providers like Anthropic, OpenAI, and Google are fundamentally reshaping how developers access their models. Subscription tiers (like Anthropic's Claude Pro or ChatGPT Plus) are increasingly off-limits to apps and app harnesses that wrap these models for end users. Instead, developers must use the API, which charges per-token -- often 2-10x the cost of a subscription for the same usage.

This creates a hard problem for autonomous AI agents:

- **API costs compound fast** — An agent making thousands of calls autonomously can run up bills that would bankrupt a subscription model.
- **Traditional billing doesn't work** — Credit cards and monthly invoices assume predictable usage. Agents don't fit that pattern.
- **Managed API services add markup** — Proxy services that handle billing add another layer of cost, taking a cut on every call.

The result: developers need a way to let agents pay for API calls instantly, per-call, with granular cost control -- and they need it now.

RustyClawRouter addresses this with payment settling directly on Solana. Agents hold USDC, compute the exact cost upfront, and pay atomically per request -- no subscriptions, no overpaying, no mystery bills.

## The Problem

AI agents are becoming autonomous. They run 24/7, make decisions, and call APIs without a human clicking buttons. But when an AI agent needs to use a large language model (like GPT-4 or Claude), it hits a wall: how does it pay?

- **Credit cards** require a human to sign up and manage billing.
- **API keys** require an account, and if the key leaks, someone else runs up the bill.
- **Monthly subscriptions** don't make sense for agents that might make 3 calls or 3 million.

RustyClawRouter (RCR) solves this. An AI agent just needs a Solana wallet with some USDC (a dollar-pegged stablecoin), and it can pay for any LLM API call instantly, with no account and no human in the loop.

## The Payment Flow

Every request follows three steps:

### Step 1: Ask the Price

The agent sends a request to RCR -- for example, "I want to ask Claude a question about quantum physics." RCR looks at the request, figures out which model to use and how much it will cost, and responds with the price.

This is like walking up to a vending machine and seeing the price on the display before you insert your coins.

### Step 2: Sign the Payment

The agent sees the price (say, $0.002625 in USDC) and signs a payment transaction on Solana. This is a cryptographic signature -- the agent authorizes the transfer from its wallet but the money moves on the Solana blockchain, not through RCR's servers.

The agent sends the signed payment back to RCR along with the original request.

### Step 3: Verify and Deliver

RCR verifies that the payment is valid and settled on Solana. If everything checks out, RCR forwards the request to the LLM provider (OpenAI, Anthropic, Google, etc.), gets the response, and sends it back to the agent.

The whole process takes under a second.

## What Is x402?

RCR uses a protocol called **x402**. The name comes from HTTP status code 402 -- "Payment Required" -- which was reserved in the original HTTP spec for future use but never standardized.

x402 adds a payment layer to HTTP, the same protocol that powers every website. Think of it like how HTTPS added encryption to HTTP -- x402 adds payments. When a server needs payment, it returns a 402 response with the price and accepted payment methods. The client pays and retries the request. The server verifies and serves the resource.

This means any HTTP client (including AI agents) can pay for API calls using a standard protocol, without custom integrations for each provider.

## Smart Routing

Not every question needs the most expensive model. Asking "what's 2+2?" doesn't require the same horsepower as "write me a compiler."

RCR's smart router analyzes each request across 15 dimensions -- things like whether the request contains code, requires reasoning, uses technical terminology, or needs creative writing. Based on this analysis, it picks the best model from five providers (OpenAI, Anthropic, Google, xAI, DeepSeek) for the job.

The agent sends one request and gets the optimal response. No need to know which provider to call or which model to pick.

## The 5% Fee

RCR charges a 5% platform fee on every request. If the LLM provider charges $0.0025, the total cost to the agent is $0.002625.

This fee covers:

- **Infrastructure**: Servers, databases, caching, and monitoring that keep the gateway running.
- **Smart routing**: The analysis engine that picks the best model for each request.
- **Provider management**: Maintaining connections to multiple LLM providers, handling rate limits, retries, and failovers.
- **Payment verification**: Validating Solana transactions and preventing replay attacks.

Every response includes a cost breakdown showing the provider cost, the platform fee, and the total -- full transparency.

## Trustless Escrow

For expensive operations -- long multi-turn conversations, batch processing, or image generation -- paying upfront for the maximum possible cost is wasteful. What if the conversation ends early?

RCR offers a trustless escrow option:

1. **Deposit**: The agent deposits the maximum estimated cost into an escrow account on Solana. This account is controlled by a smart contract (an on-chain program), not by RCR.
2. **Service delivery**: RCR processes the request and tracks the actual cost.
3. **Settlement**: After service delivery, the smart contract releases the actual cost to the gateway operator and automatically refunds the remainder to the agent.
4. **Timeout protection**: If the gateway never claims the funds (say, the server goes down), the agent can reclaim the full deposit after a timeout period. No trust required.

The escrow is entirely on-chain. The gateway software can claim only what was earned, and the agent can always get unclaimed funds back. Neither party needs to trust the other.

## Enterprise Features

Organizations can manage multiple agents and team members through RCR's enterprise features:

- **Teams**: Group agents and team members. Set spend limits per team or per hour.
- **API keys**: Authenticate programmatically instead of with wallet signatures.
- **Audit logs**: Every action is logged -- who did what, when, and how much it cost.
- **Budget controls**: Set hourly and total spend limits. Agents that hit the limit get a clear error, not an unexpected bill.
