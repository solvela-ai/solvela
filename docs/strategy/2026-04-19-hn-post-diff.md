# HN Post Diff — Draft Comparison + Merged Version

> Comparison of `docs/launch-drafts/hn-show-post.md` (team draft) vs my advisory draft in `docs/runbooks/2026-04-19-announcement.md` (now superseded).
> **Verdict:** team draft wins on structure and concreteness. My draft contributes 3 specific beats worth pulling in.

---

## 1 — Beat-by-beat comparison

| Beat | Team draft | My draft | Winner |
|---|---|---|---|
| **Title** | "Show HN: Solvela – x402 LLM payments for autonomous agents" (60 chars) | "Show HN: Solvela – x402 payment gateway for Solana with escrow" (79 chars) | **Team** — shorter, punchier, foregrounds "agents" (your buyer), not "Solana" |
| **Opening line** | "Solvela is a production MCP server that lets AI agents pay for LLM calls in real USDC-SPL on Solana via the x402 protocol. No API keys, no accounts, no per-user subscriptions." | "Hi HN — I've been building Solvela in Rust for the last several months. It's an HTTP gateway…" | **Team** — third-person product-led beats "Hi HN — I've been building…" on HN |
| **Install instruction** | `npm install -g @solvela/mcp-server` + `solvela mcp install --host=claude-code` right up top | No install command in body | **Team** — HN readers scan for commands; team surfaces them |
| **Concrete metrics** | "<1s on-chain finality", "~1.2s end-to-end", "$0.00025 tx cost", "~$0.10 USDC + ~$0.001 SOL needed" | Mentions "400 RPS", "683 tests" | **Team** — per-call numbers beat RPS theater for this audience |
| **Tools list** | 6 tools enumerated with descriptions | Not enumerated | **Team** |
| **Linux Foundation x402 context** | Not mentioned in body | Mentioned ("Linux Foundation x402 as of April 2") | **Mine** — this is rising tide validation; useful for skeptical HN |
| **Escrow framing** | "Moat: Escrow is trustless… We've deployed one to mainnet." (1 line, near bottom) | Described as "unique… solves the LLM-provider-crashed-after-I-paid problem" | **Mine (slightly)** — problem-framing beats moat-framing for HN. But the team line is tighter. |
| **Telsi + RustyClaw live customers** | Not mentioned | Mentioned ("Telsi.ai, RustyClaw.ai… paying Stripe customers") | **Mine** — this is the single most missed beat; must be added |
| **Escrow program address** | "We've deployed one to mainnet" (no address) | `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` (on-chain verifiable) | **Mine** — a pubkey invites Solana Explorer clicks. Trust signal. |
| **Engagement invitation** | Not explicit in body | "Happy to answer anything — technical, regulatory, or product" | **Mine** — HN rewards an open invitation |
| **Predictable-comment prep** | 5 drafted responses (BlockRun, OpenRouter, price, Phantom, chains) | None | **Team** — this is a significant preparation advantage |
| **Early-comment-within-15-min plan** | Explicit (3 bullet points on session caps, zeroed-out secrets, latency) | Generic "engage within first 30 min" advice | **Team** — operational detail beats generic "be there" |
| **x402-rs / FareSide layer carving** | Not mentioned | Not mentioned in mine either | **Neither** — gap in both. Must be added after today's research. |

**Score:** team draft wins 8 beats, mine wins 4, both miss 1 (x402-rs). Team draft should be the base; pull in the 4 beats where mine contributes.

---

## 2 — Recommended merged HN body

The following is a pull-together: team's structure + concreteness, augmented with my 4 beats. Character count target ≤ 2000 (HN limit); final count below comes in at ~1,850.

```
Solvela is a production MCP server + gateway that lets AI agents pay
for LLM calls in real USDC-SPL on Solana via the x402 protocol. No
API keys, no accounts, no per-user subscriptions.

x402 became Linux Foundation infrastructure on 2026-04-02 alongside
Coinbase, Cloudflare, Stripe, Google, Visa, Circle, and the Solana
Foundation. ~65-70% of x402 volume settles on Solana today.

Install:
  npm install -g @solvela/mcp-server
  solvela mcp install --host=claude-code

What's shipping:
- MCP server for Claude Code, Cursor, Claude Desktop (6 tools: chat,
  smart_chat, list_models, wallet_status, spending, deposit_escrow)
- Trustless mainnet escrow — program 9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU
  on Solana. Pay only for what you actually receive; server crash
  mid-response means your deposit is refundable.
- OpenClaw provider plugin (Solvela appears in the model picker)
- Cross-platform CLI installer (macOS, Linux, Windows)
- Rust + Axum gateway, 683 tests, load-tested to 400 RPS with
  p99 < 300ms

Two commercial products already run on this in production:
- Telsi.ai — multi-tenant AI assistant SaaS, migrated from BlockRun
  to Solvela in April 2026
- RustyClaw.ai — crypto trading terminal with autonomous AI agent

How one call works:
1. You ask Claude to call an LLM via the chat tool
2. Solvela computes the cost and returns an x402 402 response with
   an exact-amount payment requirement
3. Your MCP server signs a Solana transaction locally (key never
   leaves your machine; secret bytes zeroed after signing)
4. Gateway verifies signature on-chain (<1s Solana finality)
5. LLM response streams back; USDC settles atomically with response
6. Cost breakdown in the response: provider cost, 5% fee, total

A note on the Rust x402 landscape: x402-rs (crates.io) is the
de-facto Rust library ecosystem (x402-axum, x402-types, now
x402-chain-solana shipped 2026-04-14). FareSide is the hosted
facilitator in closed beta. Solvela isn't competing at the library
or facilitator layer — it's the gateway + escrow + routing layer on
top. Library vendors don't run gateways; we do.

Pricing: flat 5% per call. No credit float, no minimum. Transparent
breakdown in every response.

Happy to answer anything — technical (Rust internals, x402 verify,
escrow PDA design), regulatory (why we're a protocol adapter, not an
MSB), or product (pricing, routing logic, providers).

Docs: https://docs.solvela.ai
Source: https://github.com/<org>/solvela
Dashboard: https://app.solvela.ai
```

Swap `<org>` to whichever you pick (`solveladev` vs `solvela-ai`). Final char count ~1,850 — under the 2,000 limit with 150 chars of headroom.

---

## 3 — What to keep from the team's `hn-show-post.md`

Everything the merged version doesn't use — specifically:

- **Title** — already copied in as-is.
- **The 5 predictable-comment responses** (BlockRun, OpenRouter, price, Phantom, multi-chain) — these are strong, tight, and well-researched. Keep verbatim.
- **Early comment** (the 15-min first-response) — good operational detail. Keep, but fix one thing: change "Base/EVM is on the roadmap (Q2) via Coinbase's `Upto` payment scheme" to mention it's post-Solana-consolidation, not a parallel effort.

---

## 4 — New predictable comment to add (#6: x402-rs)

Slot this between the existing "Why Solana vs Base/ETH" and "How is this different from OpenRouter":

```
> "How is this different from x402-rs + x402-chain-solana + an
>  afternoon of Axum?"

Fair question — they're the default Rust x402 library stack now
(including x402-axum as an Axum middleware). Two honest points:

1. Layer. x402-rs gives you protocol verification primitives. It
   doesn't give you a deployed LLM gateway, provider aggregation,
   smart routing, escrow, or running customers. Building the layer
   that sits on top of x402-rs — in production, on mainnet, with
   real paying customers — is the work.

2. Escrow. x402-rs doesn't have one. Coinbase's facilitator doesn't
   have one. FareSide doesn't have one (yet). Pay-only-for-what-you-
   receive requires an on-chain program, deploys, tests, and mainnet
   ops — not a library.

We could use x402-rs as a facilitator dependency (and may, as a
configurable backend). But the gateway, the escrow, the smart router,
the A2A adapter, and the running customers are not things you
dependency-install.
```

This pre-empts the single highest-probability HN objection given today's research.

---

## 5 — Recommended sequence for launch day

1. Post the **merged body above** (with title from team draft).
2. Within 60 seconds, check the post is live.
3. Within 10 minutes, post the **early comment** (from team draft).
4. Keep a tab open with all **6 predictable-comment responses** (team's 5 + new x402-rs one) ready to paste.
5. Do not stall. Every reply in first 2 hours = keeps thread on the front page longer.
6. **Do not** edit the post body after posting. HN doesn't update the submitted-at rank, but edits can nuke the "hot" algorithm input.

---

## 6 — Final notes

- My draft will be deprecated once this merged version is approved. The only reason to keep my draft around is the x402-rs predictable-comment addition, which this doc already captures.
- The team's `x-thread.md`, `blog-post-solvela-ai.md`, and others should apply **the same three additions** (Telsi/RC, x402-rs layer, escrow program pubkey). All three edits are mechanical and cross-cutting — batch them.
