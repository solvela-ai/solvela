# Blog Post: Solvela Public Launch

**Status:** DRAFT. Do NOT publish. User triggers after private testing.

**Target:** solvela.ai/blog  
**Audience:** AI/Solana developers, agents, engineers building with LLM APIs  
**Length:** ~1,100 words

---

## Headline Options (Pick One)

1. **"Agents Should Pay for Their Own LLM Calls — Here's How (on Solana)"**
2. **"Open x402 Protocol Meets Solana: Pay-per-Call LLM Access Without Accounts"**
3. **"Escrow-First LLM Payments: Solvela Brings x402 to Claude Code, Cursor, and OpenClaw"**

---

## Article

# Agents Should Pay for Their Own LLM Calls — Here's How (on Solana)

For months, we've watched AI agents hit the same wall: they need to call LLM APIs, but they have no API keys, no accounts, and no credit cards. A self-hosted agent can't use OpenAI's auth system. A wallet-backed agent needs to prove it can pay per-call, not per-month.

Today, we're shipping Solvela public — a production-grade MCP server that lets agents pay for LLM calls directly, in real USDC-SPL on Solana, via the x402 protocol.

No API keys. No accounts. No per-user subscriptions. One transaction signature per call.

## The Problem: API Keys Don't Work for Autonomous Agents

The current model assumes humans manage credentials. You get an API key from OpenAI, drop it in an env var, and make calls. But agents live in a different model:

- **Self-hosted agents** (Claude in your terminal, or running in a container) have no auth backend to mint new keys.
- **Multi-agent systems** (spawning sub-agents) can't safely distribute a single master key without risking exfiltration.
- **Agentic networks** (agents talking to agents) need a payment rail, not a shared secret.

API keys are a human credential model. Agents need a payment model.

## The Solution: x402 on Solana

The x402 protocol, now part of Linux Foundation infrastructure, defines a standard for pay-per-call APIs: if a call costs money, the server returns HTTP 402 with a cost breakdown, the client signs a payment proof, and retries with that proof in the headers.

Solana is the x402 carrier for LLM payments. It's where the volume is — 65–70% of all x402 transactions settle on Solana. And it's fast: finality in under a second.

Solvela is the gateway that bridges x402 + Solana + your favorite LLMs.

## What Shipped

### 1. MCP Server for Claude Code, Cursor, Claude Desktop

```bash
npm install -g @solvela/mcp-server
solvela mcp install --host=claude-code
```

One command. Your Claude Code gets 6 new tools:

- **chat** — Send a prompt, pay in USDC-SPL
- **smart_chat** — Let Solvela's 15-dimension router pick the best model
- **list_models** — See all 26+ models and pricing
- **wallet_status** — Check balance and session spending
- **spending** — View cumulative spend with budget enforcement
- **deposit_escrow** — Top up a trustless escrow deposit (optional)

[SCREENSHOT: Claude Code showing the `chat` tool in action, with a cost breakdown in the response]

### 2. Trustless Escrow on Mainnet

Every payment goes through an Anchor program deployed to Solana mainnet.

Normally: you sign a transfer, USDC moves, the gateway calls the LLM. If the response times out, your money is gone.

With escrow: your USDC is locked on-chain. The gateway claims only when the LLM response completes. If it fails, your deposit is refundable.

This is a real product moat. Competitors can't match it without an on-chain program + audit + months of trust-building.

[SCREENSHOT: Escrow contract explorer showing a live deposit PDA]

### 3. OpenClaw Provider Plugin

For OpenClaw users, Solvela appears as a first-class model provider — not a tool you have to call manually.

```bash
npm install @solvela/openclaw-provider
openclaw models list | grep solvela  # Shows Solvela models
openclaw chat --model solvela/claude-sonnet-4 "your prompt"
```

Per-call x402 signing happens transparently via a `wrapStreamFn` hook. The agent sees Solvela models in the picker and doesn't have to know about the payment layer.

### 4. CLI Installer for All Platforms

The `solvela` CLI (Rust, cross-compiled via `cargo-dist`) handles the install boilerplate:

```bash
solvela mcp install --host=cursor --wallet=<pubkey> --budget=10.00
```

Writes the right config file for your host, with env vars pre-filled. Works on macOS, Windows, Linux, Linux ARM64.

[SCREENSHOT: Terminal showing `solvela mcp install` output]

## Your First Call

Here's what happens when you use the `chat` tool:

1. **Request:** You ask Claude for a hello-world Rust program.
2. **Compute cost:** Solvela checks your model choice and estimates input/output tokens. ~$0.002 total.
3. **Request payment:** Your local MCP server signs a Solana transaction with your wallet key (kept locally, never sent to Solvela).
4. **Verify on-chain:** The gateway checks the signature against the Solana ledger. Takes <1s.
5. **Call LLM:** OpenAI gets the request. Returns the completion.
6. **Settle:** If escrow mode, claim the pre-locked deposit. If direct mode, transfer the USDC atomically.
7. **Stream back:** Response streams to Claude. Cost breakdown included.

[SCREENSHOT: Cost breakdown showing input tokens, output tokens, 5% platform fee, total]

Total end-to-end latency: ~1.2 seconds (computation + x402 signature + on-chain verification + LLM call).

## Why Solana

- **x402 volume:** 65–70% of the ~154M cumulative x402 txns live here.
- **Settlement speed:** Sub-second finality. x402 needs fast feedback loops.
- **Cost:** Solana txns cost ~$0.00025, vs ~$1 on Ethereum. Mattering for high-frequency agent payments.
- **Narrative:** Solana Foundation is positioning "agentic infrastructure" as core strategy. We're betting they're right.

## Architecture: Why Rust

Solvela's gateway is written in Rust + Axum. Competitors (BlockRun, Skyfire, Bankr) are TypeScript or Python.

Why it matters:

- **Performance:** 400 RPS ceiling measured under load. TypeScript single-threaded event loops hit ~100 RPS.
- **Concurrency:** Tokio async runtime handles thousands of simultaneous payment verifications. No thread-per-request overhead.
- **Correctness:** Compile-time guarantees on thread safety and memory ownership. Payment systems need that.

This isn't marketingspeak — it's the difference between a gateway that scales and one that falls over during high-agent-activity windows.

## The Smart Router

Solvela's smart router classifies incoming prompts across 15 dimensions (code presence, reasoning markers, technical depth, etc.) and routes to the model tier best-suited to the task:

- **eco:** Claude Haiku (cheapest, for simple tasks)
- **auto:** Claude Sonnet (balanced, default)
- **premium:** Claude Opus (deepest reasoning)
- **free:** Completely free tier (if you set it)

Example: "Write hello world in Rust" → eco tier → Haiku → $0.0002. A request about "design a distributed consensus algorithm" → premium tier → Opus → $0.015. Same tool; different model chosen by the router.

Result: agents using `smart_chat` pay ~30–40% less on average for the same quality response.

## Pricing & Economics

- **Platform fee:** 5% per call. Settled per-call in USDC-SPL.
- **No hidden fees:** Solvela takes its 5%, the rest goes to the LLM provider. Transparent breakdown in every response.
- **No minimum balance:** Fund a wallet, sign transactions. No account login, no approval process.
- **Escrow option:** Lock funds on-chain, pay only on completion. Default when available.

For comparison: OpenRouter charges 3–5% + a credit model (top-up, sit unused). Skyfire charges 8–15%. Solvela is 5%, per-call, with escrow as the default.

## What's Next

**Phase 1 (shipped):** MCP server, OpenClaw provider, CLI installer.

**Phase 2 (April):** Multi-wallet adapters (Phantom deeplink, hardware wallet support). Solvela today requires a local keypair; we're adding options.

**Phase 3 (Q2):** EVM support. Base mainnet via Coinbase's `Upto` scheme. Same x402 flow, different chain.

**Phase 4 (Q3):** OpenClaw Skills (LLM guidance on when to use Solvela vs other providers). Nosana integration (decentralized GPU inference on Solana — fully on-chain stack).

## Try It

### Prerequisites

- A Solana wallet with ~$0.10 USDC and ~$0.001 SOL for rent
- Node.js 18+

### Install

```bash
npm install -g @solvela/mcp-server
export SOLANA_WALLET_KEY="your-base58-key"
export SOLANA_RPC_URL="https://api.mainnet-beta.solana.com"
solvela mcp install --host=claude-code
```

### First Call

Open Claude Code. The `chat` tool appears in your tool picker. Send a prompt.

First 5 users to sign up get $5 of free USDC credit. [LINK TO SIGNUP, if applicable]

---

## Links

- **Docs:** https://docs.solvela.ai
- **GitHub:** https://github.com/solveladev/solvela
- **Dashboard:** https://app.solvela.ai

---

**Posted:** [AUTO-FILLED: current date]  
**Author:** Solvela team  
**Questions?** reach out to hello@solvela.ai
