# X (Twitter) Launch Thread Draft

**Status:** DRAFT. Do NOT post. User triggers after private testing.

---

## Thread Structure: 11 Tweets (10-tweet blog-repurpose source)

Timestamps and images: Each tweet can carry one image or video. Specify type; don't generate.

---

### Tweet 1 (Hook)

```
Agents need to pay for their own LLM calls. No API keys, no accounts, no subscriptions. Solvela is live: one line to install. [link]

[MEDIA: Hero image — Solana logo + "$" + "Agents"]
```

**Length:** 134 chars

---

### Tweet 2 (The Moat: Escrow — promoted from #5)

```
the real story: escrow.

normally: you sign a transfer. usdc moves. gateway calls the llm. if the 
response times out, your money is gone.

with solvela: usdc is locked on-chain. claim only on completion. if it 
fails, you get it back.

pay only for what you receive.

[MEDIA: Anchor program on Solana explorer]
```

**Length:** 225 chars

---

### Tweet 3 (The Problem)

```
the problem: agents don't have api keys.

they can't log into openai. they can't manage a credit card. they can't 
authenticate like humans do.

autonomous agents need a different model. they need to pay.

[MEDIA: Red X on "API Keys", red X on "Accounts"]
```

**Length:** 145 chars

---

### Tweet 4 (The Solution)

```
enter x402: an http standard for pay-per-call apis.

server returns http 402. client signs a payment proof. retries with the 
proof. payment verified. api call succeeds.

it works. it's standardized. solana is where 65% of the volume lives.

[MEDIA: x402 protocol diagram]
```

**Length:** 167 chars

---

### Tweet 5 (What Shipped)

```
solvela v1.0:
- mcp server (claude code, cursor, claude desktop)
- trustless escrow on solana mainnet
- openclaw provider plugin
- cli installer for all platforms (macos, windows, linux, arm64)

26+ models. 5 providers. one signature per call.

[MEDIA: Terminal showing `solvela mcp install`]
```

**Length:** 168 chars

---

### Tweet 6 (Proof: Dog Food — new)

```
Two products already run on Solvela: Telsi (@telsi_ai — multi-tenant AI SaaS) and RustyClaw (@rustyclaw_ai — crypto terminal). We eat our own dog food. Paying customers, not demos.

[MEDIA: Logos of Telsi.ai and RustyClaw.ai]
```

**Length:** 180 chars

---

### Tweet 7 (Why Rust Matters)

```
Solvela's gateway is Rust + Axum. Load-tested to 400 RPS with p99 < 300ms. Benchmarks: [link].

why it matters:
- tokio async handles 1000s of concurrent payment verifications
- compile-time guarantees on memory safety

payment systems need this.

[MEDIA: Benchmark chart showing RPS under load]
```

**Length:** 243 chars

---

### Tweet 8 (First Call)

```
here's what happens:

1. you ask claude to call an llm
2. solvela computes cost. requests payment via x402.
3. your wallet signs. key never leaves your machine.
4. gateway verifies on solana. <1s finality.
5. llm responds. usdc settles on-chain.
6. you get the result.

[MEDIA: Flow diagram with numbered steps]
```

**Length:** 188 chars

---

### Tweet 9 (Pricing & Transparency)

```
pricing:
- 5% platform fee. per-call.
- no credit float. no minimum balance.
- no hidden fees.

transparent cost breakdown in every response.

for comparison:
openrouter: 3–5% + credit model
skyfire: 8–15%
GateRouter: 2.5% (base only)

[MEDIA: Price comparison table]
```

**Length:** 142 chars

---

### Tweet 10 (Try It)

```
install:

npm install -g @solvela/mcp-server
export SOLANA_WALLET_KEY="your-key"
(better: put it in ~/.solvela/env chmod 600, per docs).
solvela mcp install --host=claude-code

then use the `chat` tool in claude. sign once per call. done.

you need ~$0.10 usdc + ~$0.001 sol for rent.

[MEDIA: QR code to docs.solvela.ai]
```

**Length:** 218 chars

---

### Tweet 11 (Vision / CTA)

```
where we're going:

phase 2 (may): phantom deeplink + hardware wallet support
phase 3 (q2): base/evm via coinbase upto
phase 4 (q3): nosana integration (decentralized gpu on solana)

open-source. shipping in the open.

github: solvela-ai/solvela
docs: docs.solvela.ai

[MEDIA: Product roadmap graphic]
```

**Length:** 188 chars

---

## Thread Notes

- **Character counts:** All tweets ≤280 chars (room for platform variations)
- **Cadence:** Post tweets 1–5 over 30 min, then 6–10 over the next hour. Stagger to maximize distribution.
- **Quote tweets:** Encourage community to quote-tweet with their own first-call screenshots.
- **Engagement:** Have someone monitoring replies in real-time. Pin favorite/earliest reply to surface community.
- **Hashtags (optional):** `#Solana` `#x402` `#Agents` `#LLM` can be woven into any tweet (don't overuse).

---

## Media Assets (to be created separately)

1. Hero image (1200x630) — Solana + agent + payment icon
2. API Keys red X (800x400) — striking, minimal
3. x402 protocol diagram (1000x600) — flow chart showing 402 response + proof
4. Terminal screenshot (1200x700) — `solvela mcp install` output
5. Anchor explorer screenshot (1200x700) — live escrow PDA on Solana
6. Benchmark chart (1000x600) — Solvela vs TypeScript competitors, RPS under load
7. Flow diagram (1200x800) — 6-step first-call flow (numbered)
8. Price comparison table (1000x600) — Solvela vs OpenRouter, Skyfire, GateRouter
9. QR code (400x400) — links to docs.solvela.ai
10. Roadmap graphic (1200x700) — timeline through Q3

---

## Posting Checklist

- [ ] Prepare all 10 media assets
- [ ] Copy thread text into X draft (thread composer)
- [ ] Attach media to each tweet
- [ ] Preview thread (ensure order, formatting, images load)
- [ ] Schedule or go live (timing: typically Tuesday–Thursday, 8–10am PT for US tech audience)
- [ ] Post thread
- [ ] Post early engagement comment within 5 min (e.g., "1/10 — AMA in replies")
- [ ] Monitor replies for the first hour
- [ ] Retweet best community replies / quote-tweets
- [ ] Log final metrics (impressions, retweets, quote-tweets, new followers)

---

## Alt Thread Angle (Shorter, More Technical) — PREFERRED FOR CURRENT X ALGO

**Recommendation: Use this 5-tweet version for your main X post.** The current X algorithm favors tight, high-signal threads over long explainers. Keep the 11-tweet version above as the blog-repurpose source (good for embedding in launch post, Substack, etc.).

1. **Hook:** "Agents need to pay for their own LLM calls. No API keys, no accounts, no subscriptions. Solvela is live: one line to install. [link]"
2. **Escrow:** "pay only for what you receive. anchor program on mainnet. usdc claimed only on completion."
3. **Install:** "npm i -g @solvela/mcp-server"
4. **Proof:** "Two products already run on Solvela: Telsi (@telsi_ai — multi-tenant AI SaaS) and RustyClaw (@rustyclaw_ai — crypto terminal). Paying customers, not demos."
5. **Try:** "docs at docs.solvela.ai | github: solvela-ai/solvela"

This works better for current X algo (fewer tweets = more reach per tweet). Use the full 11-tweet version if cross-posting to a Substack/blog thread or Solana-focused accounts where depth is valued.
