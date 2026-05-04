# Superteam Grant — Application Draft

> Working draft. Apply at the regional Superteam — for US-based maintainers,
> https://superteamusa.com (or the Earn platform at https://earn.superteam.fun
> for bounty-style funding). Superteam applications are short by design — keep
> the submission tight; the long version goes in the linked artifacts, not the
> form.

## Why Superteam, not just the Solana Foundation

The Solana Foundation grant is filed in parallel
([`solana-foundation-application.md`](./solana-foundation-application.md)).
Superteam serves a different purpose:

- **Faster signal.** Superteam tends to respond in days–weeks; Foundation in
  weeks–months.
- **Builder-flavored.** Superteam reviewers bias toward "ships things" over
  "promising plans."
- **Smaller, focused asks.** $5k–$15k for a *specific deliverable*, not
  $50k for a portfolio of work. We ask for one thing only.
- **Regional network.** A regional Superteam award routes Solvela into local
  builder communities — Breakpoint hallway introductions, podcast spots, IRL
  hackathon judging. Worth more than the dollars over time.

## The ask (single deliverable)

**$10,000 for the public Solvela payments dashboard at `metrics.solvela.ai`.**

A live, no-login web page that shows real-time on-chain data for the Solvela
gateway:

- Total USDC routed (mainnet, all-time + last 24h + last 7d)
- Active wallets (last 24h, last 7d)
- Per-provider request volume (OpenAI, Anthropic, Google, xAI, DeepSeek)
- Per-model token volume
- Escrow program activity (deposits, claims, refunds) with Solscan links
- p50 / p95 / p99 gateway latency
- Cache hit rate, replay-rejection rate

Every number on the page links back to the underlying Solana transaction or
the gateway's Prometheus metrics endpoint. Nothing is server-rendered fiction —
if the gateway is down, the page says so.

### Why this specifically

This is the single highest-leverage piece of work that's currently unbuilt and
that compounds:

- Grant evaluators (Solana Foundation, Helius, RPC providers) verify project
  claims by checking the live numbers. A public dashboard turns "trust me" into
  "click here."
- Acquirers do the same diligence; the dashboard is essentially pre-built
  diligence material.
- It removes the maintainer from the loop on "how is the project doing"
  questions — anyone can self-serve.

## Why Solvela fits Superteam's mission

- **Real, shipping, on Solana.** Mainnet escrow program live at
  `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`. Production gateway at
  `api.solvela.ai`. P99 < 300 ms at 400 RPS sustained.
- **Public good.** Protocol crates (`solvela-protocol`, `solvela-x402`,
  `solvela-router`) and SDKs are all MIT. Anyone on Solana who wants to charge
  for an HTTP service in USDC can fork our code rather than re-derive
  `TransferChecked` discriminators from scratch.
- **No token, no DAO, no anonymous co-founders.** Solo maintainer. Email
  `kd@sky64.io`. The regulatory posture is documented at
  [`docs/product/regulatory-position.md`](../product/regulatory-position.md).
- **Concrete deliverable, concrete timeline.** This grant funds *one page* with
  a clear definition of done. No scope creep.

## Timeline

| Week | Deliverable |
|---|---|
| 1 | Domain + Vercel deploy of `metrics.solvela.ai` skeleton; data-fetch layer reading from `api.solvela.ai/metrics` and Solscan |
| 2 | Total USDC routed + active wallets + per-provider volume tiles, all linking to on-chain transactions |
| 3 | Escrow activity + per-model breakdown + latency percentiles |
| 4 | Cache + replay metrics, mobile layout, public launch post (Twitter/X + Solana Foundation forum) |

## Budget

| Item | Cost |
|---|---|
| Frontend implementation (4 weeks part-time) | $7,500 |
| Domain + Vercel + monitoring (1 year) | $500 |
| Helius RPC tier needed for the dashboard's read load (1 year) | $1,500 |
| Buffer for scope adjustments | $500 |
| **Total** | **$10,000** |

Funds received in USDC. Tracked in a public ledger.

## Public artifacts (verify these before approving)

- Repo: https://github.com/solvela-ai/solvela
- Status: https://github.com/solvela-ai/solvela/blob/main/STATUS.md
- Mainnet program: `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`
- Live gateway: https://api.solvela.ai/health
- Regulatory posture: https://github.com/solvela-ai/solvela/blob/main/docs/product/regulatory-position.md
- Licensing: https://github.com/solvela-ai/solvela#licensing
- Commercial license terms: https://docs.solvela.ai/enterprise/commercial-license

## What we will not do with this grant

- Launch a token.
- Add custodial flows or fiat conversion.
- Build features unrelated to the dashboard.
- Re-apply to Superteam in the same calendar quarter for an unrelated deliverable.

---

## Submission checklist (delete before sending)

- [ ] Replace any placeholder bio text with real bio
- [ ] Confirm the Solana Foundation application has been submitted (so this can
      reference it as a parallel filing rather than a duplicate)
- [ ] Verify the regional Superteam (USA / Germany / India / etc.) matches the
      maintainer's location
- [ ] Add a 60-second screen recording of the gateway accepting a real USDC
      payment, hosted on Loom or YouTube unlisted
- [ ] If applying via Earn for bounty-style funding instead, restructure as a
      bounty: "Build the Solvela payments dashboard — $10k bounty, 4-week
      delivery, payable on merge to `main`"

## After submission

Append `## Submitted` with the date and grant officer contact.
Append `## Outcome` when the result arrives, with lessons for the next
application.
