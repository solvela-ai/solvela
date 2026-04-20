# X (Twitter) Launch Thread Draft

**Status:** DRAFT. Do NOT post. User triggers after private testing.

---

## Thread Structure: 10 Tweets, ~80 char avg per tweet

Timestamps and images: Each tweet can carry one image or video. Specify type; don't generate.

---

### Tweet 1 (Hook)

```
agents should pay for their own llm calls.

with usdc. on solana. no keys. no accounts.

solvela is live. one line to install. ship it.

[MEDIA: Hero image — Solana logo + "$" + "Agents"]
```

**Length:** 96 chars

---

### Tweet 2 (The Problem)

```
the problem: agents don't have api keys.

they can't log into openai. they can't manage a credit card. they can't 
authenticate like humans do.

autonomous agents need a different model. they need to pay.

[MEDIA: Red X on "API Keys", red X on "Accounts"]
```

**Length:** 145 chars

---

### Tweet 3 (The Solution)

```
enter x402: an http standard for pay-per-call apis.

server returns http 402. client signs a payment proof. retries with the 
proof. payment verified. api call succeeds.

it works. it's standardized. solana is where 65% of the volume lives.

[MEDIA: x402 protocol diagram]
```

**Length:** 167 chars

---

### Tweet 4 (What Shipped)

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

### Tweet 5 (The Moat: Escrow)

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

### Tweet 6 (Why Rust Matters)

```
solvela's gateway is rust + axum. competitors are typescript.

why it matters:
- 400 rps ceiling under load (ts single-threaded tops out ~100)
- tokio async handles 1000s of concurrent payment verifications
- compile-time guarantees on memory safety

payment systems need this.

[MEDIA: Benchmark chart showing RPS under load]
```

**Length:** 191 chars

---

### Tweet 7 (First Call)

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

### Tweet 8 (Pricing & Transparency)

```
pricing:
- 5% platform fee. per-call.
- no credit float. no minimum balance.
- no hidden fees.

transparent cost breakdown in every response.

for comparison:
openrouter: 3–5% + credit model
skyfire: 8–15%
gaterrouter: 2.5% (base only)

[MEDIA: Price comparison table]
```

**Length:** 142 chars

---

### Tweet 9 (Try It)

```
install:

npm install -g @solvela/mcp-server
export SOLANA_WALLET_KEY="your-key"
solvela mcp install --host=claude-code

then use the `chat` tool in claude. sign once per call. done.

you need ~$0.10 usdc + ~$0.001 sol for rent.

[MEDIA: QR code to docs.solvela.ai]
```

**Length:** 173 chars

---

### Tweet 10 (Vision / CTA)

```
where we're going:

phase 2 (may): phantom deeplink + hardware wallet support
phase 3 (q2): base/evm via coinbase upto
phase 4 (q3): nosana integration (decentralized gpu on solana)

open-source. shipping in the open.

github: solveladev/solvela
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

## Alt Thread Angle (Shorter, More Technical)

If you prefer a tighter, more technical thread (5–7 tweets instead of 10):

1. **Hook:** "agents need to pay for api calls, on-chain, without api keys."
2. **What:** "x402 protocol on solana. solvela is the gateway."
3. **Install:** "npm i -g @solvela/mcp-server"
4. **Escrow:** "pay only for what you receive. anchor program on mainnet."
5. **Why:** "autonomous agents. trustless settlement. no accounts."
6. **Try:** "docs at docs.solvela.ai"

This works better if you're speaking to a technical Solana audience and want to skip the explainer.

**Recommendation:** Use the full 10-tweet version for maximum reach. Use the 5-tweet version if reposting to Solana-focused accounts.
