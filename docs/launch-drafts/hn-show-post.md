# Hacker News "Show HN" Post Draft

**Status:** DRAFT. Do NOT submit. User triggers after private testing.

---

## Post Details

**Site:** news.ycombinator.com  
**Post type:** Show HN  
**Format:** Text + URL  

---

## Title Options (Pick One — HN title ≤ 80 chars)

1. "Show HN: Solvela – Pay-per-call LLM access on Solana via x402"  
2. "Show HN: x402 LLM payments for autonomous agents"  
3. "Show HN: Escrow-first LLM gateway on Solana mainnet"  

**Chosen:** "Show HN: Solvela – x402 LLM payments for autonomous agents"  
**Length:** 60 chars ✓

---

## Text Body (≤2000 chars for HN)

```
Solvela is a production MCP server that lets AI agents pay for LLM calls 
in real USDC-SPL on Solana via the x402 protocol. No API keys, no 
accounts, no per-user subscriptions.

One line to install:
  npm install -g @solvela/mcp-server
  solvela mcp install --host=claude-code

What's live:
- MCP server for Claude Code, Cursor, Claude Desktop (6 tools: chat, 
  smart_chat, list_models, wallet_status, spending, deposit_escrow)
- Trustless escrow on Solana mainnet (Anchor program) — pay only for 
  what you receive
- OpenClaw provider plugin (Solvela models in the picker)
- CLI installer for all platforms (macOS, Windows, Linux)

How it works:
1. You ask Claude to call an LLM via the `chat` tool
2. Solvela computes the cost and requests payment via x402
3. Your wallet signs the transaction (local signing, key never leaves 
   your machine)
4. Gateway verifies the signature on Solana (takes <1s)
5. LLM call completes, USDC settles on-chain, response streams back

Cost: 5% platform fee per call. Transparent. Per-call settlement.

Why it matters: Autonomous agents need to pay for APIs, but they don't 
have API keys or credit cards. x402 is the standard for this. Solana is 
where 65–70% of x402 transactions live.

Moat: Escrow is trustless. Competitors can't match it without an audited 
on-chain program. We've deployed one to mainnet.

Repo: https://github.com/solveladev/solvela
Docs: https://docs.solvela.ai
Live gateway: https://api.solvela.ai
```

**Length:** ~1,350 chars ✓

---

## Suggested Early Comment (Post Within 15 Min of Submission)

This comment should be posted by the submitter (you) within 15 minutes of the post going live. Hacker News culture rewards this — signals the author is present and engaged.

```
Happy to answer questions. A few quick points:

1. "Why Solana?" — x402 volume lives here (65–70% of ~154M cumulative 
   txns). Finality is <1s. Tx cost is $0.00025 vs ~$1 on Ethereum. For 
   agents making 100s of calls/day, this matters.

2. "What stops my agent from draining my wallet?" — The `deposit_escrow` 
   tool gates this. You set `SOLVELA_MAX_ESCROW_DEPOSIT` ($5 default per 
   call) and `SOLVELA_MAX_ESCROW_SESSION` ($20 default per session). An 
   adversarial loop can't exceed the session cap regardless of how many 
   times it calls the tool. Session state is persisted to disk.

3. "Is the key safe on my machine?" — Yes, we zero-out secret bytes 
   after signing (using `volatile-secret` under the hood). Keys never 
   leave your machine. Never sent to Solvela servers. We default to 
   escrow mode (pay-only-for-what-you-receive) for extra safety.

4. "What's the latency hit?" — End-to-end (computation + signature + 
   on-chain verification + LLM call) is ~1.2s. The x402 verification 
   itself adds <100ms. Measurably slower than a direct API call, but 
   agents don't have API keys, so it's the only option.

5. "Does this work with other chains?" — V1 is Solana only. Base/EVM is 
   on the roadmap (Q2) via Coinbase's `Upto` payment scheme. The 
   `PaymentVerifier` trait is chain-agnostic, so the add is mechanical.

We're building this in the open. GitHub issues welcome.
```

**Length:** ~1,000 chars

---

## Predictable HN Comments (Prepare Responses)

### Comment 1: "Why Solana and not Base / ETH L2?"

**Predicted:** HN will ask why Solana vs the "bigger" ecosystem.

**Your response:**

```
Two reasons:

1. Volume: x402 lives on Solana. 65–70% of the ~154M cumulative txns. 
   Base is emerging (Coinbase launched `Upto` in 2026-02), but Solana's 
   where the action is right now.

2. Finality + cost: Solana txns finalize in <1s and cost $0.00025. Ethereum 
   L2s are cheaper now (Base is ~$0.001), but finality is still 12–15s. For 
   agents making 100s of calls/day, <1s feedback loops matter.

That said: we architected the `PaymentVerifier` trait to be chain-agnostic. 
Base support is in the roadmap (Q2). We're not anti-EVM; we're shipping 
where the x402 infrastructure is strongest first.
```

---

### Comment 2: "How is this different from OpenRouter?"

**Predicted:** "OpenRouter already lets you use 300+ models for less." (True, but different model.)

**Your response:**

```
Good question. OpenRouter and Solvela solve different problems:

**OpenRouter:**
- You get an API key, top up a credit balance, make calls
- Account model: you manage your balance per-session
- Works great for humans

**Solvela:**
- Agents sign transactions, pay per-call on-chain
- No accounts, no API keys, no credential management
- Works great for autonomous agents that own wallets

OpenRouter's account model assumes a human manages the credentials. 
Solvela's payment model assumes agents own their wallet.

Both can coexist. An agent might use OpenRouter for some calls and 
Solvela for others, depending on the context.
```

---

### Comment 3: "Is this just a wallet pass-through for another gateway?"

**Predicted:** "You're just a wrapper around OpenAI's API with a payment layer on top."

**Your response:**

```
We handle three layers:

1. **Payment** (our innovation) — x402 protocol on Solana. Trustless 
   escrow. Per-call settlement.

2. **Routing** — 15-dimension smart router that classifies prompts 
   (code presence, reasoning depth, technical terms, etc.) and picks the 
   best model tier (eco/auto/premium/free) for the task. Not just a 
   passthrough to one provider.

3. **Provider aggregation** — We speak OpenAI-compatible API on the 
   client side, but proxy to 5 providers (OpenAI, Anthropic, Google, 
   xAI, DeepSeek) on the backend. You get model choice.

So: yes, we call LLM APIs. But we add escrow, routing, and multi-provider 
support. Not a transparent wrapper.

(The real value is escrow. Competitors can't match it without an audited 
on-chain program.)
```

---

### Comment 4: "What about Phantom / hardware wallet support?"

**Predicted:** "I don't want to store a key on my machine."

**Your response:**

```
Fair point. V1 ships with local keypair signing only.

V2 (May) will add:
- Phantom deeplink support (opens wallet app, returns signed tx)
- Hardware wallet adapters (Ledger, etc.)

The MCP protocol doesn't have a great story for interactive auth flows 
(it's stdin/stdout), so Phantom deeplinks require some ceremony (user 
must approve each payment in the wallet app). We're building it anyway.

In the meantime: if you run Claude Code locally and trust your machine, 
local keypair signing is safe (we zero-out secret bytes after signing). 
If you're paranoid: use escrow mode — it caps your per-session exposure 
to $20 by default.
```

---

### Comment 5: "How does the 5% fee compare to X?"

**Predicted:** "GateRouter is 2.5%, OpenRouter is ~5% + credit float, Skyfire is 8–15%."

**Your response:**

```
Honest comparison:

- **GateRouter:** 2.5%, but Base-only (not Solana)
- **OpenRouter:** 3–5% + credit model (you top up, some sits unused)
- **Skyfire:** 8–15%
- **Solvela:** 5%, per-call, no credit float

We're in the middle on price. The escrow is where the value is: you 
only pay for completed responses. If a stream fails mid-response, you 
get your escrow back. Competitors can't do this without an audited 
on-chain program.

If you need the cheapest price, GateRouter on Base is your play. If you 
want to guarantee pay-only-for-what-you-receive, Solvela's escrow is 
unique.
```

---

## Submission Checklist

- [ ] Create HN account (if new) and verify email
- [ ] Draft post with chosen title
- [ ] Paste body text (exact copy from above)
- [ ] Include URL: https://solvela.ai or https://github.com/solveladev/solvela
- [ ] Submit on HN (https://news.ycombinator.com/submit)
- [ ] Wait ~1–2 min for post to go live
- [ ] Post the "Suggested Early Comment" within 15 minutes
- [ ] Monitor thread; respond to top-level comments
- [ ] Have FAQ answers ready (see Predictable Comments above)
- [ ] Log reactions for post-mortem (HN traffic, tone, key objections)
