> **⚠️ SUPERSEDED 2026-04-19** — The canonical launch copy lives in `docs/launch-drafts/` (`hn-show-post.md`, `x-thread.md`, `blog-post-solvela-ai.md`, `cursor-directory-submission.md`, `openclaw-docs-pr.md`, `solana-foundation-grant-update.md`, `anthropic-mcp-registry.json`). Use those.
>
> **Kept here for:** the sequencing / timing advice (HN-before-Twitter, first-24h response plan, tracking table) is still useful supplementary material and isn't duplicated in the launch-drafts.

# Launch Announcement Copy (advisory draft — superseded)

> **Fire condition:** All six publish steps in `2026-04-19-sdk-publish.md` are DONE and verified.
> **Do not stagger.** One tweet, one HN post, one changelog, one Discord, all within the same 4-hour window. Momentum compounds; drips don't.
> **Scope:** Solvela ecosystem launch. Sky64 is not mentioned anywhere.

---

## 0 — The positioning line (write this once, use everywhere)

> **Solvela is an open-source x402 payment gateway for AI agents on Solana, with mainnet escrow and MCP plugins for Claude Code, Cursor, and OpenClaw. Two commercial products — Telsi.ai and RustyClaw.ai — run on it today.**

Memorize it. That line goes in every single announcement in some form. Do not write a different positioning sentence for each channel. Consistency is credibility.

---

## 1 — Hacker News "Show HN" post

HN has hard rules: title must start with "Show HN:", max 80 chars, no marketing superlatives ("amazing", "revolutionary" → instant flag).

### Title (pick one)

**Option A (preferred — feature-led):**
> `Show HN: Solvela – x402 payment gateway for AI agents on Solana with escrow`
(79 chars)

**Option B (proof-led):**
> `Show HN: Solvela – Solana LLM gateway running Telsi and RustyClaw in prod`
(77 chars)

**Pick A.** HN rewards technical clarity over social proof in the title. Put proof in the body.

### URL field

`https://solvela.ai` (or `https://github.com/solvela-ai/solvela` if that reads better)

### Body

```
Hi HN — I've been building Solvela in Rust for the last several months. It's an HTTP gateway that lets AI agents pay for LLM API calls with USDC on Solana, using the x402 protocol (HTTP 402 Payment Required, now a Linux Foundation standard as of April 2).

The short version: instead of giving an agent your OpenAI API key (leak risk) or a credit card (requires a human), the agent signs a Solana transaction per call. The gateway verifies the tx on-chain, proxies to one of 5 LLM providers, and returns the response. Settlement is sub-second. No accounts, no subscriptions, no minimum spend.

What's live today:
- Gateway: api.solvela.ai (Rust / Axum, 683 tests, 400 RPS ceiling)
- 5 providers: OpenAI, Anthropic, Google, xAI, DeepSeek
- Mainnet Anchor escrow program for trustless "pay only for what you receive" flows — deposit max cost to a PDA, gateway claims actual, difference refunds automatically, or you reclaim after timeout if the server dies
- SDKs published today: crates.io (Rust CLI), PyPI (Python), npm (TypeScript + Vercel AI SDK provider), Go modules, MCP server for Claude Code / Cursor / Claude Desktop / OpenClaw
- Two commercial products running on it in production: Telsi.ai (multi-tenant AI assistant SaaS, migrated from BlockRun in April) and RustyClaw.ai (crypto trading terminal with AI agent)

Why I'm posting: this category is consolidating fast (BlockRun, Skyfire, Catena, Bankr, GateRouter all shipping x402 variants), and I wanted the open-source + escrow angle on record. The escrow specifically is unique — nobody else has a mainnet-deployed, tested on-chain escrow program for LLM payments, and it solves the "LLM provider crashed after I paid" problem in a way direct settlement can't.

Happy to answer anything — technical (Rust internals, x402 verification, escrow PDA design), regulatory (why we're a protocol adapter and not an MSB), or product (pricing model, routing logic, provider circuit breakers).

Docs: https://docs.solvela.ai
Source: https://github.com/solvela-ai/solvela
Escrow program: 9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU (mainnet)
```

### HN timing

- **Best post window:** Tuesday–Thursday, 8:00–10:00 AM US Eastern. Avoid Friday (dies over weekend), Monday (ignored).
- **Don't stack with other noise.** Post Solvela alone; don't also submit Telsi or RustyClaw the same day.
- **Engage within first 30 min.** Answer every reply fast for the first 2 hours. HN ranking is engagement-sensitive early.

---

## 2 — X / Twitter thread

9 tweets. First one must stand alone because that's what gets quote-tweeted.

### Tweet 1 (hook)

> AI agents need a way to pay for API calls without accounts, credit cards, or subscriptions.
>
> Today I'm open-sourcing Solvela — an x402 payment gateway that lets agents pay per-call with USDC on Solana. Plus a Solana mainnet escrow program for trustless "refund what you don't use."
>
> 🧵

### Tweet 2 (what's shipping)

> What shipped today:
>
> 📦 Rust CLI on crates.io (`cargo install solvela-cli`)
> 🐍 Python SDK on PyPI (`pip install solvela-sdk`)
> 📦 TS SDK on npm (`@solvela/sdk`, `@solvela/ai-sdk-provider`)
> 🐹 Go SDK (`go get github.com/solvela-ai/solvela-go`)
> 🔌 MCP plugin for Claude Code, Cursor, Claude Desktop, OpenClaw

### Tweet 3 (the protocol)

> x402 = HTTP status 402 "Payment Required" turned into a real protocol. Joined the Linux Foundation on April 2 alongside Coinbase, Cloudflare, Stripe, Google, AWS, Visa, Circle, Solana Foundation.
>
> Solana handles ~65-70% of x402 volume. It's the right chain.

### Tweet 4 (the escrow — your moat)

> The unique piece: Solvela has a mainnet Anchor escrow program (`9neDHouXgEgHZDde5Sp...`).
>
> Agent deposits max cost → gateway claims only actual cost → difference refunds automatically → if gateway dies, agent reclaims after timeout.
>
> No trust required. The math is on-chain.

### Tweet 5 (proof)

> Two commercial products are already running on Solvela in production:
>
> • Telsi.ai — multi-tenant AI assistant SaaS, migrated from BlockRun to Solvela in April
> • RustyClaw.ai — crypto trading terminal with an autonomous AI agent
>
> Both are paying customers of my own infrastructure. Dogfood is real.

### Tweet 6 (MCP for agent dev)

> If you're building in Claude Code, Cursor, or OpenClaw:
>
> `solvela mcp install --host=claude-code`
>
> One command. Your agent now has a Solvela-routed LLM tool that pays per call in USDC. Zero config files.

### Tweet 7 (Rust + 683 tests — for technical audience)

> Gateway is Rust/Axum. 683 tests. 5 providers (OpenAI, Anthropic, Google, xAI, DeepSeek). 15-dimension smart router that picks the best model per request. Fluid Compute-style per-call billing.
>
> Load tested at 400 RPS. SLO: p99 < 300ms.

### Tweet 8 (pricing — be direct)

> Pricing: 5% platform fee on every call. That's it. No subscriptions, no minimum spend, no hidden markup. Every response includes a cost breakdown — provider cost, fee, total — in USDC.
>
> If you use escrow, the contract enforces "only pay for what you receive."

### Tweet 9 (call to action)

> Docs: https://docs.solvela.ai
> GitHub: https://github.com/solvela-ai/solvela
> Dashboard: https://app.solvela.ai
>
> Try it with devnet USDC in 5 minutes. If you break something, reply to this thread and I'll fix it.

### Who to tag / mention

**DO NOT @-tag in the thread itself.** Reduces reach algorithmically. Instead, after the thread is live, quote-retweet it and tag strategically:

- `@solana` (Solana foundation account)
- `@circle` (USDC issuer)
- `@coinbase` (x402 founding member)
- `@anthropicai` (you use Claude — they sometimes amplify)
- Helius, Phantom, Backpack if you have relationships

### Timing

Post the thread within 30 minutes of the HN submission. The two reinforce each other in the first hour.

---

## 3 — docs.solvela.ai changelog entry

Add to `docs/changelog/` as `2026-04-19-public-sdks.mdx`:

```markdown
---
title: "Public SDK Release: Solvela is now open source"
date: 2026-04-19
tag: major
---

# Public SDK Release

Today we're publishing the Solvela SDK suite to public registries and open-sourcing the gateway.

## What's available

| Package | Registry | Install |
| --- | --- | --- |
| `solvela-cli` | crates.io | `cargo install solvela-cli` |
| `solvela-sdk` | PyPI | `pip install solvela-sdk` |
| `@solvela/sdk` | npm | `npm install @solvela/sdk` |
| `@solvela/ai-sdk-provider` | npm | `npm install @solvela/ai-sdk-provider` |
| `github.com/solvela-ai/solvela-go` | Go modules | `go get github.com/solvela-ai/solvela-go` |
| `@solvela/mcp-server` | npm | `npx -y @solvela/mcp-server` |
| `@solvela/openclaw-provider` | npm | `npm install @solvela/openclaw-provider` |

## MCP one-liner install

```bash
solvela mcp install --host=claude-code    # Claude Code
solvela mcp install --host=cursor         # Cursor
solvela mcp install --host=claude-desktop # Claude Desktop
solvela mcp install --host=openclaw       # OpenClaw
```

## Source

- Gateway: [github.com/solvela-ai/solvela](https://github.com/solvela-ai/solvela)
- Escrow program: mainnet program ID `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`

## Stability

All packages ship as `0.1.0`. Public API is usable and tested, but breaking changes are possible before 1.0.0. Pin your versions.

---

Questions? `support@solvela.ai` or GitHub Issues.
```

---

## 4 — Discord / community post (short, if you have a channel)

```
🚀 Solvela SDKs are public.

cargo install solvela-cli
pip install solvela-sdk
npm i @solvela/sdk

Full list + MCP installer: docs.solvela.ai
Thread: [x.com link]
HN: [hn link]

Happy to answer anything in #help.
```

---

## 5 — Email to people who should hear first (BEFORE public post)

Send this ~2 hours before the HN submission goes live, to 5–15 people max. These are folks who might amplify but will feel patronized if they find out from HN.

**Subject:** Heads up — Solvela goes public this morning

```
Hey {name},

Small heads up — I'm pushing Solvela live as a public open-source project today. SDKs go to crates.io / PyPI / npm / Go / MCP in the next couple hours, HN submission follows, thread right after.

If you want to take a look or boost it: [link to thread once live]. No pressure either way.

Happy to talk about what I've learned building on Solana + x402 if you're ever up for a call.

— Kenneth
```

Who gets this email:
- Anyone from Solana Foundation you've talked to
- Anyone at Helius / Phantom / Circle who knows you
- Any prior investors or advisors
- The strongest 2–3 technical friends who'd boost a serious launch
- **NOT** any competitor contact. Obviously.

---

## 6 — What to do in the first 24 hours after launch

- **Hour 0–2:** Respond to every HN comment within 10 minutes. Every one.
- **Hour 2–6:** Answer Twitter replies. Ignore sarcasm; engage technical skepticism substantively.
- **Hour 6–24:** Monitor for issues. Expect: 1–2 GitHub issues, 1–2 "how do I..." questions, maybe 1 reproducible bug.
- **Day 2:** Write a follow-up thread covering what went wrong and what you fixed. Transparency is a launch force multiplier.

**Do NOT:**
- Repost the thread. It's dead after 24 hours.
- Re-submit to HN if it flopped. Post-mortem and try a different angle in 3–4 weeks.
- Reply to negative takes emotionally. Draft in a doc, sit on it 2 hours, then decide.

---

## 7 — Tracking

One-page spreadsheet or Notion table. Capture for 30 days:

| Metric | Day 1 | Day 7 | Day 30 |
|---|---|---|---|
| HN upvotes + position | | | |
| Twitter thread impressions / likes / reposts | | | |
| GitHub stars | | | |
| Package downloads (npm, PyPI, crates) | | | |
| New wallet addresses hitting gateway | | | |
| Support tickets | | | |
| Inbound "can I integrate" messages | | | |
| Competitor reactions | | | |

That table tells you whether the launch actually moved anything, and gives you numbers to use in the exit conversation two months from now.
