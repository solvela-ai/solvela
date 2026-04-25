# Solvela Go-Live & Survival Playbook

> **Scope note (2026-04-19):** This playbook covers the **Solvela + Telsi + RustyClaw** ecosystem. **Sky64 is explicitly out of scope** — it is Kenneth's separate local networking/IT services business with long-term local clients, not part of the Solvela ecosystem or any future sale/transition. Earlier drafts that referred to Sky64 as a "holding brand" or parent org are superseded by this note.
>
> **Date:** 2026-04-19
> **Author:** Advisory session (Claude)
> **Status:** Strategic reference — not a tactical plan. Treat as a menu, not a checklist.

---

## 0 — Honest frame

You already shipped. Gateway is in prod on `api.solvela.ai`, escrow is on mainnet, Telsi is paying rent on it, RustyClaw is producing real USDC volume, 683 tests green. "Going live" for you is not a launch button — it's a **distribution + operations + trust** problem with a ~60–90 day window to convert technical lead into category position before BlockRun, Skyfire, Bankr, or OpenRouter close it.

Treat Solvela as the **platform bet**, and RustyClaw + Telsi as **living proof** (your own dogfood + your only unbiased case studies). Sky64 is separate and stays separate.

---

## 1 — Repo & org architecture (do NOT merge)

Keep them separate. Solvela, Telsi, and RustyClaw are three distinct products that share SDK-level infrastructure.

| Repo | Owner | Visibility | Why |
|---|---|---|---|
| `solvela` | `solvela-ai` GH org | **public eventually** (core gateway + SDKs) | Open-core is your distribution lever; you want issues/PRs |
| `rustyclaw` | `rustyclaw-ai` GH org | **private** | Contains Stripe + exchange creds + paid-user logic |
| `clawstack` (Telsi) | `telsi-ai` GH org | **private** | Multi-tenant SaaS, customer data model |

**Why not monorepo:** blast radius (a Telsi migration bug can't brick `api.solvela.ai`), licensing (open-source Solvela later without exposing trading code), commit cadence (Solvela moves weekly, Telsi feature-gated, RustyClaw daily), and — most importantly — Solvela must look like neutral infrastructure. If it lives in the same repo as a trading terminal it's "Kenneth's stack," not a standard.

**Shared library strategy:** publish `solvela-ts` / `solvela-python` / `solvela-go` to public registries. Telsi and RustyClaw consume them as external packages, same as any third party would. That forces the API surface to stay clean.

**Action items:**
1. Create/confirm `solvela-ai`, `telsi-ai` GH orgs. Keep yourself as admin on all.
2. ~~Archive `rcr-docs-site` and `RustyClawRouter` remotes; redirect READMEs.~~ **Done 2026-04-21.** `rcr-docs-site` had no remote — archived locally to `~/projects/archive/rcr-docs-site-2026-04-21.tar.gz`. `RustyClawRouter` never existed on disk or GitHub — item was moot.
3. Optional: a single `solvela-ai/ecosystem` repo with one README mapping how Solvela relates to Telsi + RustyClaw.

---

## 2 — Go-live sequence (T-30 → T+90)

### T-30 to T-14: close remaining gates

- **Publish SDKs** — PyPI (`solvela`), npm (`@solvela/sdk`), crates.io (`solvela-sdk`), Go mod tag. Each with a working `HelloWorld` in README that pays real USDC on devnet. Cheapest traction move you have.
- **Benchmark doc** — Solvela vs BlockRun vs Skyfire: end-to-end latency, p99 under load, cost per call. Publish as `docs/benchmarks/`.
- **Status page** — `status.solvela.ai`. Instatus ($20/mo) or roll from Prometheus.
- **Escrow upgrade authority** — decide: keep, multisig (Squads), or revoke. Regulators and buyers will ask. Multisig is the right middle.
- **Legal consult** — attorney review of `docs/product/regulatory-position.md`. $3–8k for a 2-hour MSB/FinCEN opinion. Don't skip. California DFAL lands July 2026.
- **TOS / Privacy / AUP** at `/legal/*`. Stripe-style structure; disclaim LLM output warranty.
- **Insurance quote** — E&O + tech liability. Vouch/Embroker/Hiscox. $1.5–3k/yr.

### T-14 to T-0: launch infrastructure

- **Marketing site polish** — escrow on the fold. H1: *"Trustless USDC payments for AI agents. No accounts. Escrow guarantees refunds."*
- **Pricing page** — explicit: 5% direct, 5% escrow (consider 2% escrow / 5% direct tiering later).
- **Docs quickstart** — first paid devnet call in < 5 minutes. Measure this.
- **MCP plugin** — `@solvela/router` for Claude Code / Cursor / Claude Desktop / OpenClaw.
- **Launch-day comms drafts** — HN, Twitter, Solana Foundation submission, Linux Foundation x402 listing, /r/solana, Product Hunt queue.

### T-0: launch day

Frame as an **agent-framework integration announcement**, not a product launch:

> "Solvela: open-source x402 gateway with mainnet escrow + MCP plugin for Claude Code, Cursor, ElizaOS. Live case studies: Telsi.ai (multi-tenant SaaS, 6 months in prod) + RustyClaw.ai (crypto terminal, paying users). 5 providers, Rust, 683 tests."

### T+1 to T+30: first users, first outages

Expect one bad deploy, one provider rate-limit surprise, one wallet drain scare, one regulatory DM, one "can you add OpenRouter" request. Runbooks before they happen (§8).

### T+30 to T+90: defend the category

- Weekly SDK shipping cadence.
- Monthly benchmark update.
- ONE integration partner: ElizaOS, ARC/Rig, Nosana, or Kuzco.
- **T+60 decision:** Base/EVM expansion or not?

---

## 3 — Telsi + RustyClaw as proof

Publish specific receipts (with your own consent — you own all three):

- "Telsi has processed N paid LLM calls through Solvela since 2026-04-07 migration from BlockRun. Zero custodial incidents. Median settlement 1.4s."
- "RustyClaw agents spend $X/day in USDC-SPL via Solvela, end-to-end autonomous."
- One dashboard screenshot per case.

**Hide:** Telsi tenant identities, RustyClaw user wallets, exact MRR.

**Tagline:** "We eat our own dog food. Two commercial products run on Solvela today. You can run yours tomorrow."

---

## 4 — Support system

Single unified inbox. Tool: **Help Scout** ($25/mo) or **Plain** ($50/mo, better for dev tools).

| Address | Purpose | SLA |
|---|---|---|
| `support@solvela.ai` | public questions, bugs | P1 4h / P2 24h / P3 72h |
| `security@solvela.ai` | vulns (in `.well-known/security.txt`) | P0 1h always |
| `abuse@solvela.ai` | reports | 24h |
| `legal@solvela.ai` | takedowns, LE requests | 48h |
| `sales@solvela.ai` | enterprise, grants, partnerships | 24h |
| `ops@solvela.ai` | alerts inbox | real-time |
| `press@solvela.ai` | PR, podcasts | 5 days |
| `hello@solvela.ai` | everything else | 72h |

Mirror for `telsi.ai` and `rustyclaw.ai` (at least `support@`, `security@`, `abuse@`).

### Triage tiers

- **P0** — Payment / custody / security. 15-min ack. No agent.
- **P1** — API down / wrong responses. 1h ack.
- **P2** — SDK bugs, docs errors. 24h. Agent-first, human review.
- **P3** — Feature asks, "how do I…". 72h. Agent can fully resolve.

---

## 5 — Automation & help agents

Help agents live in **a separate repo** (`solvela-ai/agent-ops`), call other services via API, never share a process with the gateway.

Stack: Node or Python worker → Temporal / Trigger.dev → Claude Sonnet 4.6 via Solvela (dogfood receipts).

| Agent | Input | Action |
|---|---|---|
| `support-triage` | Help Scout webhook | Classify P0-P3, draft reply, label, page on P0/P1 |
| `docs-answer` | Ticket + RAG over `docs/` | Propose answer; one-click approve |
| `pr-triage` | GitHub webhook | Label, ask for repro, suggest reviewer |
| `incident-scribe` | Pager event | Open channel, timestamped log, post-mortem skeleton |
| `fee-payer-watch` | `rcr_fee_payer_balance` | Auto-topup or page |
| `escrow-claimer-health` | Claim queue depth | Alert + draft RCA hypothesis |
| `billing-receipts` | Solana RPC | Reconcile recipient wallet vs usage DB |
| `signup-funnel` | New wallet in DB | Enrich, drop into CRM, welcome email |
| `release-drafter` | Git tag push | Changelog to Discord + Twitter |
| `competitor-watch` | RSS/Twitter | Weekly digest to inbox |

**Do NOT build** an agent that can move USDC out of your recipient wallet, merge to `main`, deploy to Fly.io, or send outbound email without human approval — not in T+90 window.

---

## 6 — Email architecture

For each domain (`solvela.ai`, `telsi.ai`, `rustyclaw.ai`):

- **SPF:** `v=spf1 include:_spf.resend.com include:_spf.google.com -all`
- **DKIM:** Resend + Google keys
- **DMARC:** `p=quarantine; rua=mailto:dmarc@…; pct=100; adkim=s; aspf=s` — start `none`, escalate over 2 months.
- **MTA-STS + TLS-RPT:** enable.
- **BIMI:** skip until revenue supports it.

**Sender split:**
- **Resend** for app email → `noreply@mail.solvela.ai` (separate subdomain!)
- **Google Workspace** for human email → `you@solvela.ai`, `support@…`

Minimum Workspace: 2 paid seats + free groups/aliases.

**Regulator/partner signals:**
- `security.txt` at `/.well-known/security.txt`
- Published vuln disclosure policy (`/security`)
- `abuse@` actually monitored

---

## 7 — Hiring (trigger-based)

| Trigger | Hire | Cost |
|---|---|---|
| First outage AFK | Fractional on-call buddy (founder swap) | equity or $2–4k/mo |
| >5h/wk on tickets | Part-time technical support | $1.5–3k/mo |
| MRR > $15k sustained 3mo | DevRel engineer (contract → FTE) | $8–12k/mo |
| >1 prod incident/week × 4wks | SRE / platform eng (FTE) | $160–220k |
| Enterprise deal > $50k ARR | Fractional CFO + real accounting | $1–2k/mo |
| 12 mo or institutional raise | Cofounder / COO | equity |

**Do NOT hire for 6+ months:** marketing manager, salesperson, designer, community manager, "head of growth."

---

## 8 — Admin & ops prep

### Legal
- Entity review. LLC ok if bootstrapping; **C-corp Delaware** if raising or selling.
- TOS / Privacy / AUP / DPA template.
- MSB opinion letter (input: `regulatory-position.md`).
- California DFAL monitoring.

### Financial
- Business bank: Mercury or Brex.
- Stripe per fiat-charging product (Telsi, RustyClaw). Solvela stays USDC-only.
- **Daily USDC sweep** from hot recipient wallet → Squads multisig. Leave ~7 days of ops liquidity hot. Single biggest thing that kills crypto-native companies.
- Accounting: Pilot/Bench/QuickBooks + bookkeeper after month 3.

### Security
- Quarterly secret rotation calendar.
- 1Password Teams ($8/user).
- Hardware 2FA on all admin (GitHub, Fly, Vercel, Google, Cloudflare, registrar, Stripe, Mercury).
- Quarterly backup restore drill.
- **Skip SOC2** until a paying enterprise requires it.

### Incident runbooks (`docs/runbooks/`)
1. Gateway 5xx spike
2. Fee-payer SOL empty
3. Provider outage
4. Stripe dispute/chargeback
5. Suspected wallet compromise
6. GDPR/CCPA data-subject request
7. Law enforcement / subpoena

---

## 9 — Risk ledger

| # | Risk | Likelihood | Damage | Mitigation |
|---|---|---|---|---|
| 1 | OpenRouter ships x402 | Med | Existential on acquisition | Pre-written response; escrow + agent-native positioning |
| 2 | Fee-payer SOL dry weekend | High | Immediate outage | `fee-payer-watch` + auto-topup |
| 3 | Provider key leak/rotation break | High | Per-provider outage | Quarterly rotation |
| 4 | BlockRun cuts to 2% | Med | Price pressure | Don't match; tier on escrow |
| 5 | CA DFAL against you | Low-Med | CA geofence | Attorney opinion + pre-built geofence |
| 6 | Escrow program bug | Low | Catastrophic | Audit before escrow becomes headline |
| 7 | Solana network event | Med-High | Transient | Circuit breaker + degraded-mode |
| 8 | **Single point of failure: you** | Very high | Total | Fractional on-call + runbooks + agents |
| 9 | Telsi/RC incident stains Solvela | Med | Trust hit | Subdomain + "customer of" framing |
| 10 | Burnout in month 4 | High | Total | Business risk, not personal. Fractional help before you need it. |

---

## 10 — Honest survival assessment

**Probability of still existing as a going concern in 18 months, given this playbook executed: ~45–55%.** Much better than median crypto-infra bets.

**Movers:**
- **+15–20%** if in next 60 days: SDKs public, MCP plugin, one agent-framework integration, one new paying enterprise > $2k/mo
- **+10%** if Solana Foundation grant or Circle Alliance conversation
- **+5%** if fractional-hire on-call buddy before you think you need one
- **−20%** if OpenRouter ships x402 and you don't post counter-positioning in 7 days
- **−15%** if escrow ships an unaudited bug — fatal, not survivable
- **−25%** if you try to out-ship this alone and burn out at month 4–5

**Shape of "surviving":** Solana-native, escrow-first, agent-framework gateway. $3–8M ARR niche by late 2027. 3–5 person company. Not nothing.

**Shape of "winning":** requires an event you don't control. Don't plan for it; stay positioned to catch it.

**#1 lever:** distribution, not engineering. Another integration / benchmark / registry publish beats another feature.

---

## 11 — This week's actual actions

1. Publish `solvela-ts` to npm. Publish `solvela-python` to PyPI. Today.
2. Put `security.txt` at `/.well-known/security.txt` on `api.solvela.ai`.
3. Email 5 M&A-capable attorneys for a 2-hour MSB opinion quote. Pick one within 7 days.
4. Set up `support@solvela.ai` forwarder + Help Scout trial.
5. Write `docs/runbooks/fee-payer-empty.md`. One page.
6. Decide escrow upgrade authority: keep, multisig, or revoke. Write decision down.
7. Homepage copy to **"Trustless USDC payments for AI agents. Escrow included."** Demote "15-dim smart router" to sub-bullet.

Everything else is a month away.
