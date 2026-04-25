# Solvela Ecosystem — Founder's Exit Playbook

> **Scope note:** This playbook covers selling the **Solvela + Telsi + RustyClaw** ecosystem (the three products that share Solvela infrastructure). **Sky64 is explicitly out of scope** — it is Kenneth's separate local networking/IT services business and stays with him regardless of what happens to the other three products. All references to "the ecosystem" below mean **only Solvela + Telsi + RustyClaw**.
>
> **Date:** 2026-04-19
> **Status:** Founder has decided to begin moving toward a sale while continuing to operate the ecosystem. This is a menu of options and process steps, not a committed plan.

---

## 0 — What you actually have for sale

Three possible sale shapes, ranked by my recommendation:

| Shape | What's in it | Likely buyer type | Value band |
|---|---|---|---|
| **Whole ecosystem** ✅ recommended | Solvela gateway + escrow + SDKs + MCP plugins + Telsi + RustyClaw + all related domains (solvela.ai, telsi.ai, rustyclaw.ai) | Strategic crypto-infra | $1.5–5M |
| **Solvela only** (keep Telsi/RC) | Gateway + protocol + SDKs + plugins + `solvela.ai` | LLM gateway/payments buyer | $750k–3M |
| **Piece sale** | Telsi separately, RustyClaw separately, Solvela separately | Different buyers per piece | Sum $800k–2.5M, takes 2–3 parallel processes |

**Recommendation: sell as the whole ecosystem.** Telsi + RustyClaw are your revenue + proof story. Strip them out and Solvela alone is an unaudited Rust codebase with zero MRR — a $300k acqui-hire. Together they are "platform with two shipping commercial customers and live Stripe MRR."

**Sky64 stays.** Do not put it in any data room, CIM, or teaser.

---

## 1 — Buyer universe (tier-ranked)

### Tier 1 — Most likely strategic fit

| Buyer | Why they care | Entry point |
|---|---|---|
| **Helius** | Solana infra leader; agentic/payments adjacency | Warm intro via Mert or BD |
| **Syndica / Triton / QuickNode** | Solana infra consolidation | Cold OK |
| **Circle Ventures / Circle Alliance** | USDC distribution; x402 strategic. BlockRun is already in portfolio — double-edged. | Partnerships team; Jeremy Allaire's orbit |
| **Phantom / Backpack** | Agentic wallet story; Solvela slots as payment rail | Product/BD |
| **Coinbase Ventures / CDP** | x402 founding coalition; CDP wants an LLM gateway | Coinbase CDP team |
| **Solana Labs / Solana Foundation** | They announced building one — you already did | Foundation grants first, then BD |

### Tier 2 — Plausible tuck-in

| Buyer | Angle |
|---|---|
| **BlockRun.ai** | Direct competitor consolidating category. Low probability, fast close if it happens |
| **Skyfire** | $9.5M seed, Solana expansion is their missing piece |
| **Catena Labs** | $18M a16z seed; your clean reg posture fits |
| **OpenRouter** | Nuclear scenario — they buy to skip building x402 |
| **Nevermined** | EU-based; want Solana presence |

### Tier 3 — Longer shots

Stripe, Cloudflare, Google, Jupiter, Jito, Pyth, ElizaOS/Virtuals/ARC (latter usually equity-only).

### Tier 4 — Non-corporate exits (the "benefit people" paths)

1. **Solana Foundation asset transfer + grant.** $100–400k cash + consulting, plus permanent credit. ~80% probability if pursued seriously.
2. **Open-source + sunset.** Publish Apache-2.0, walk. $0, maximum karma.
3. **Hand to a successor team.** Equity swap (5–15%) for a promising team.
4. **Run down + keep domains.** `solvela.ai` stays as historical project site.

### Do not waste time on

- Private equity (deal size too small, wants $5M+ EBITDA)
- Traditional SaaS acquirers (crypto spooks their risk teams)
- Vapor "web3 roll-ups"

---

## 2 — Realistic valuation (sober)

Comps anchor: crypto/AI infra acqui-hires 2024–2026 trend $500k–$3M, $5–8M for strong codebase + engineer. SaaS MRR multiples crypto-adjacent: 3–6x ARR. Pre-traction strategic infra (Solvela shape): ~$400k × FTE-years saved at optimistic end.

### Outcome distribution

| Outcome | Probability | Value |
|---|---|---|
| No exit in 12 months | ~45% | $0 cash, keep everything + optionality |
| Acqui-hire (solo) | ~25% | $500k–$1.2M over 2–3 yrs with retention |
| Asset sale (no you) | ~15% | $750k–$2M upfront |
| Strategic acquisition (you + team) | ~10% | $1.5–5M, 1–3 yr earnout |
| Home-run strategic | ~5% | $5–15M, low probability unless competitive fire |

**Plan for $1–2M. Ready for $500k. Don't anticipate $5M.**

### Multipliers you control

- **+30–50%** publishing SDKs before pitching
- **+20–40%** if Telsi or RC shows >$5k verifiable MRR at close
- **+50–100%** for having a **second bidder** (most valuable single factor)
- **+20%** published escrow audit
- **−30%** if upgrade authority still on personal keypair — move to Squads multisig before first conversation

---

## 3 — Process timeline (4–9 months typical)

### Phase A — Readiness (4–6 weeks, before any outreach)

**Legal & corporate**
- Form **Delaware C-corp** if not already; assign all IP (code, trademarks, domains, Fly apps, Vercel projects, GitHub repos) into it. Buyers will not touch a personal-asset deal.
- Hire an **M&A lawyer** (Cooley, Gunderson, Orrick, or Goodwin's emerging-company group). $15–30k across the deal.
- **IP assignment from every contributor ever.** Single missing signature kills deals.
- **Third-party license inventory.** No AGPL. Flag any GPL or patent-encumbered code.

**Financial**
- 12 trailing months of clean books (Pilot/Bench/bookkeeper).
- **Separate USDC treasury** with on-chain history. Buyers fear commingled personal/business wallets.
- **CPA pre-read** on asset-vs-stock sale. Non-negotiable.

**Technical diligence prep**
- **Escrow audit.** Neodyme / OtterSec / Halborn. $15–40k. Without this, half of serious buyers walk.
- Skip SOC2.
- Architecture + runbooks in `docs/acquisition-pack/` branch (not main).
- Usage telemetry dashboard: wallets active, tx volume, $ settled, p99, uptime.

**Deal artifacts**
- One-page teaser (anonymous or named variant)
- CIM (15–25 pages): market, product, traction, tech, team, financials, rationale, ask
- Data room (Docsend or Ansarada)
- IOI template

### Phase B — Outreach (2–4 weeks)

1. Week 1–2: 6–10 warm intros via Solana Foundation contacts, investors, prominent Solana devs (Helius team reads DMs), x402 working group.
2. Week 2–3: Cold email to BD/Corp-Dev at Tier 1 buyers. Short. Teaser PDF only.
3. Run conversations in parallel. Goal: 3–5 in flight at once.

**Rule of three:** at least three conversations in progress before the first term sheet. Otherwise no leverage.

### Phase C — Diligence & LOI (4–8 weeks)

- Initial calls → NDA (YOUR template, not buyer's) → CIM → data room
- Management presentation (you demo)
- IOI (non-binding ranges)
- LOI (exclusive 30–45 days, no-shop)

**Do not sign an LOI you can't close with.** Read it with your lawyer AND someone who's been through one.

### Phase D — Close (3–6 weeks)

- Definitive purchase agreement
- Reps & warranties negotiation (eats time)
- Escrow/indemnity holdback (10–20% for 12–24 months — standard)
- Employment agreement if you're joining
- Asset assignment (domains, registries, contracts)
- Customer notifications (Telsi/RC paying users)
- Close

---

## 4 — Deal structure (don't get cash-whipped)

| Structure | Shape | Your take |
|---|---|---|
| **Asset sale** | Cash + 3-mo consulting | Simplest, fastest, lowest $ |
| **Acqui-hire** | Signing + 2–3yr RSUs + salary. Code comes with you | Biggest total comp; you work for them. Read non-compete carefully |
| **Stock + earnout** | $X upfront + $Y tied to milestones | Highest headline, highest risk. Earnouts often miss |

**Demand:**
- Cash at close ≥ 50% of total
- Indemnity cap ≤ 20% of price, expires ≤ 24 months
- Non-compete ≤ 12 months, narrowly scoped (e.g., "Solana x402 LLM gateway" not "crypto" or "AI")
- No personal guarantees. Ever.

---

## 5 — Don't-do list

- **Don't cold-email BlockRun first.** If they learn you're selling, they sit tight.
- **Don't sign a buyer's NDA.** Use your lawyer's template.
- **Don't name a price first.** Let them bid.
- **Don't sell to a buyer who skips the escrow audit.** They'll skip paying too.
- **Don't commingle Telsi/RC Stripe accounts into the sale without reviewing change-of-control triggers in TOS.**
- **Don't let public momentum die during diligence.** Dead GitHub = $500k haircut.
- **Don't accept a token-heavy deal.** Cash, real-corp equity, or public stock only.

---

## 6 — Non-exit exits (also honor the work)

Compatible with starting a for-profit process in parallel. If real bidder emerges you take the better deal; if not you still have a landing.

1. **Solana Foundation transfer + grant.** $100–400k + named credit. ~80% achievable in 60–90 days.
2. **Open-source + sunset.** Apache-2.0, walk away. Cleanest emotionally.
3. **Successor team handoff.** Equity swap.
4. **Run-down preservation.** Retire gateway; keep domain as project memorial.

**My honest view:** if burnout-averse and motivated by impact, option 1 is the most aligned path and genuinely achievable. Run it as Plan B in parallel with Plan A (strategic sale).

---

## 7 — Advisors to hire

| Role | Why | Cost |
|---|---|---|
| **M&A attorney** | NDA/LOI/APA, reps negotiation | $15–30k |
| **M&A CPA / tax advisor** | Asset vs stock election, state tax | $5–10k |
| **Escrow program auditor** | Valuation multiplier | $15–40k |
| **M&A broker/banker** | Skip at this size unless deal > $3M | ~8% |
| **Diligence sounding board** | Someone who's sold infra before | $500–2k/call |

**Total realistic advisory spend: $40–80k.** Best use of runway if serious about selling.

---

## 8 — First two weeks (start-line actions)

1. **Decide the shape.** Whole ecosystem recommended. Sky64 out.
2. **Form/confirm C-corp; assign all IP** for the three products.
3. **Three M&A attorney consult calls** (free 30-min intros). Pick one.
4. **Two escrow audit quotes** (Neodyme, OtterSec). Book the sooner one.
5. **Move upgrade authority to Squads multisig.**
6. **Draft one-page teaser.** Review with attorney + one trusted founder. Not to any buyer yet.
7. **Build outreach list** of 20 Tier-1/2 buyers with named contacts.
8. **One-page dashboard:** MRR (Telsi + RC), tx volume, active wallets, uptime.
9. **Publish SDKs.** Don't wait.
10. **Decide walk-away number.** Write on paper, put in drawer. Don't renegotiate with yourself mid-process.

---

## 9 — Final honest read

**Probability of any exit > $750k cash within 12 months, executed well: ~35–45%.** Median of deals that do close: **$1–2M total consideration, $600k–$1.1M cash at close, 60–75% of solo founders end up with 1–2 yr vesting component.** Not life-changing, but real. Honors the work.

**Probability a parallel Foundation / public-good handoff succeeds if for-profit doesn't:** ~80%. Solana Foundation is actively funding this category.

Run both tracks simultaneously. Let the first to close win.

---

## 10 — Next artifacts the advisor can draft

- One-page teaser (anonymous or named)
- CIM skeleton (TOC + first-page)
- Buyer target list with named BD contacts
- Solana Foundation grant / handoff proposal
- Cold email for 3 specific Tier-1 buyers

On request.
