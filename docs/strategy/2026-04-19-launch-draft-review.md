# Launch Drafts Review — 2026-04-19

> **✅ RESOLVED 2026-04-20.** All 13 items from the "Priority order for edit pass" applied. Verification greps confirmed zero remaining instances of stale content (wrong escrow address, `solveladev`, `Anchor program audited`, `gaterrouter`, `competitors top out`, `First 5 users get`). Telsi + RustyClaw proof-of-production line added across 5 files. x402-rs layer-carving paragraph added to blog + HN FAQ. `solvela-ai` GitHub org name propagated. License unified to MIT. 8 files changed in `docs/launch-drafts/` (+157/-88).
>
> **5 items remain operator-owned** (flagged in-file as TODOs, impossible to miss on final pre-submission pass):
> 1. Cursor deeplink scheme verification (`cursor://install?package=...` — scheme has changed before)
> 2. Escrow TTL constant verification (`programs/escrow/src/lib.rs`)
> 3. `escrowContractAddress` field acceptance by Anthropic MCP registry (custom field; flagged via `_submission_note`)
> 4. X thread tweet #6 media asset (Telsi + RustyClaw logos)
> 5. X thread tweet #7 benchmark link placeholder
>
> The review doc below is preserved as the specification that drove the fix pass.
>
> ---

> Review of `docs/launch-drafts/` against HANDOFF, code reality, and post-FareSide competitive context.
> **Verdict:** drafts are strong overall. **Two critical bugs** must fix before submission; several specific tightenings recommended.

---

## 🔴 CRITICAL — fix before any submission

### 1. Wrong escrow program address in 2 drafts

**Canonical (20+ references in code, HANDOFF, docs):**
```
9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU
```

**Wrong address in launch drafts:**
```
9neDHouXgEgHZDde5SpZMC7eP7UUGPvfM8v4ULiMfFJA
```

**Files to fix:**
- `docs/launch-drafts/anthropic-mcp-registry.json:111` — `escrowContractAddress`
- `docs/launch-drafts/solana-foundation-grant-update.md:26` — address inline

**Blast radius if shipped unfixed:** you'd link the Solana Foundation, Anthropic, and every agent developer installing the MCP server to a program that isn't yours. Diligent users will Solana-Explorer the address, find nothing, and assume you're vaporware.

**Action:** grep-replace globally:
```bash
grep -rln '9neDHouXgEgHZDde5SpZMC7eP7UUGPvfM8v4ULiMfFJA' docs/
# then sed or hand-edit each file
```

### 2. False audit claim in Anthropic MCP registry JSON

**Current text** (`anthropic-mcp-registry.json:112`):
> `"escrowAudit": "Anchor program audited for trustlessness. Funds held on-chain, not in Solvela custody."`

HANDOFF does **not** mention a third-party audit. You have 21 passing tests on the Anchor program and a mainnet deployment. That is not an "audit" in the sense Anthropic's registry or a buyer will interpret.

**Honest replacement:**
> `"escrowAudit": "Deployed to mainnet with 21-test suite covering deposit/claim/refund/timeout paths. Third-party audit not yet commissioned — planned before public escrow promotion."`

**Blast radius if unfixed:** lawsuit surface area; lose trust if one HN commenter googles "Solvela audit" and finds nothing.

---

## 🟡 HIGH — cross-cutting consistency

### 3. GitHub org name inconsistency

Drafts use `github.com/solveladev/solvela` throughout. HANDOFF (§ SDK publishing) implies the canonical org is `solvela-ai`. Pick one; propagate everywhere. Before submission, the actual public GitHub org must exist and host the repo at the stated URL — diligent readers click.

### 4. License inconsistency

- `cursor-directory-submission.md:29` → `license: Apache-2.0`
- `README.md` root (badge) → MIT

**Pick one, ideally MIT** (matches your SDK package.json files). Update cursor submission + any docs. If you want Apache-2.0 (more permissive for patent protection), change MIT in all package.json files — but that's a bigger decision; don't pivot at launch time.

### 5. Missing: live customer proof line (Telsi + RustyClaw)

Your single strongest "don't dismiss me" signal — *we already have paying customers running on this* — is **absent from every draft**. Telsi.ai + RustyClaw.ai are the reason Solvela isn't one of 98 x402 crates on crates.io.

Add one line to HN body, X tweet 5, blog post, Solana Foundation update, OpenClaw docs PR:

> "Two commercial products already run on Solvela in production: **Telsi.ai** (multi-tenant AI assistant SaaS, migrated from BlockRun in April) and **RustyClaw.ai** (crypto trading terminal with autonomous trading agent, paying Stripe customers)."

### 6. Missing: layer carving vs x402-rs / FareSide

After today's Rust landscape research, you know HN and technical diligence readers will ask: *"How are you different from `x402-axum` + `x402-chain-solana` + an afternoon of Axum?"*

Short honest line (add to HN preparred-comments and blog post):

> "The Rust x402 library layer is now well-populated (x402-rs, FareSide's closed beta, r402, tempo-x402). Solvela is not at that layer — we operate the LLM gateway + escrow + smart router + provider aggregation on top of x402. Library vendors don't run gateways; we do."

Adding this *before* someone raises it = credibility. Being forced to post it after = defensive.

---

## 🟢 MEDIUM — per-draft specifics

### HN post (`hn-show-post.md`)

Strong draft overall. Specific edits:

| Line | Issue | Fix |
|---|---|---|
| Body line ~40 | No Telsi/RC proof | Add `Two commercial products already run on this in production — Telsi.ai and RustyClaw.ai.` |
| Predicted comment 5 | `gaterrouter: 2.5%` typo | `GateRouter: 2.5%` |
| Predicted comment 1 | "Solana's where the action is" | Soft-edit: "Solana is where most x402 volume currently settles." Less boastful, same point. |
| Overall | No x402-rs layer-carving | Add a 6th predicted comment anticipating "how do you differ from x402-rs?" (see §6 above) |
| Tool list | "smart_chat" before "chat" would be more agent-native | Reorder |

Title: **"Show HN: Solvela – x402 LLM payments for autonomous agents"** (60 chars) is the strongest of your 3 options. Keep it.

### X thread (`x-thread.md`)

- **Tweet 1 (hook):** Lowercase + "ship it" reads like crypto-Twitter cosplay and invites derision. Try:
  > `Agents need to pay for their own LLM calls. No API keys, no accounts, no subscriptions. Solvela is live: one line to install. [link]`
- **Tweet 5 (escrow) → promote to Tweet 2.** Escrow is your moat. Lead with it.
- **Tweet 6 (Rust + 400 RPS):** The "competitors top out ~100 RPS" claim is the single most nitpickable line across all drafts. TypeScript gateways routinely exceed 100 RPS with clustering. Replace with specific Solvela number + methodology, skip competitor comparison: `Solvela's gateway is Rust + Axum. Load-tested to 400 RPS with p99 < 300ms. Benchmarks: [link].`
- **Tweet 8:** `gaterrouter` typo → `GateRouter`.
- **Tweet 9:** `export SOLANA_WALLET_KEY="your-key"` in public copy trains users to paste private keys into shells where they end up in `~/.bash_history`. Add a line: `(better: put it in ~/.solvela/env chmod 600, per docs).`
- Missing Telsi/RC dogfood tweet — insert between current Tweets 5 and 6:
  > `Two products already run on Solvela: @Telsi_ai (SaaS) and @RustyClaw_ai (crypto terminal). We eat our own dogfood. Paying customers, not demos.`
- **Alternative 5-tweet version (bottom of draft):** actually better for current X algo. Recommend using that, keeping the 10-tweet version as the blog-repurpose source.

### Blog post (`blog-post-solvela-ai.md`)

- **Headline option 1** ("Agents Should Pay for Their Own LLM Calls") is strongest. Keep.
- **Missing Telsi/RC customer section.** Add a ~150-word section titled "Proof: two products already running on this" near the top.
- **Missing x402-rs / FareSide layer discussion.** Add ~120 words as "Where Solvela sits in the x402 stack" — see §6.
- **"First 5 users get $5 of free USDC credit."** (line ~190) Is this actually decided? If not, remove. If yes, specify the mechanism (claim form? wallet address whitelist?). Unclear promos become support-ticket floods.
- **"Phase 2 (April)"** is ambiguous — today IS April. Say `Phase 2 (May 2026)` or `Next 30 days`.
- **Competitor RPS claim** (line ~136): same issue as X tweet 6. Soften.
- **Env var security:** add one-line warning near the `SOLANA_WALLET_KEY` example: `⚠️ Never commit this to git. Store in ~/.solvela/env (chmod 600) or your shell profile — not in .env files in project directories.`

### Cursor directory submission (`cursor-directory-submission.md`)

- **License mismatch** (see §4).
- **Deeplink scheme:** `cursor://install?package=...` — verify this is current Cursor deeplink format. Cursor has changed the scheme more than once; hit their docs before submitting or the PR will be rejected.
- **Required env var warning:** add a sentence to the metadata `description`: `"Requires local Solana wallet key — see docs for secure storage."` This is both useful to users and shields you from "they asked for my seed key in plaintext" complaints.
- **Deeplink URL** does not include `solvela` in the name query — add for clearer UX in Cursor's install dialog.

### OpenClaw docs PR (`openclaw-docs-pr.md`)

- **Escrow TTL (1 hour)** mentioned (line ~209). Verify this matches the current expiry slot constant in your escrow program. Your plan doc (`docs/plans/phase-8-escrow-hardening.md`) would be authoritative.
- **SOLANA_WALLET_KEY placement in config** — this ends up in a plaintext config file. Users will paste their seed key. Add a security box:
  > ⚠️ **Security:** `SOLANA_WALLET_KEY` in MCP config files grants *full spending authority* over the wallet. Use a dedicated hot wallet funded with only the USDC/SOL you're willing to risk. Keep seed keys for large balances on hardware wallets.
- **Pricing table** lists 5% flat. OK. Consider adding a row: "Escrow guarantee: no charge if gateway fails to deliver."

### Solana Foundation grant update (`solana-foundation-grant-update.md`)

- **Escrow address bug** (see §1).
- **Metrics placeholders** (lines 37-38): `[X calls, Y USDC settled]`, `[N]`. Must fill before send. If you don't have the numbers, say so plainly: *"Early-stage metrics still stabilizing — full dashboard shared separately on request."*
- **"Grant extension"** ask is vague. If you have a specific grant number, reference it. If not, say "exploration of follow-on funding" rather than "grant extension."
- **Add a "competitive context" paragraph** — the Foundation is seeing many x402 submissions. You should tell them how you differ (escrow on-chain, Rust, two customers, MCP plugin). One paragraph, ~80 words. Currently the update is very "here's what we built" and less "here's why it matters vs what else you're seeing."

### Anthropic MCP Registry JSON (`anthropic-mcp-registry.json`)

- **Escrow address** (see §1).
- **False audit claim** (see §2).
- **`worksWith`** currently `["claude-code", "claude-desktop"]` — missing Cursor (you list it in multiple other drafts). Add `"cursor"`.
- **`version: "1.0.0"`** — matches Phase 4 runbook intent. OK.
- **`"escrowContractAddress"`** — not a standard MCP registry field AFAIK. Verify Anthropic's actual schema before submitting; extra fields may be silently dropped or reject the submission.

---

## ✅ What's working well (keep)

- **Cross-draft consistency** on core tech claims (5 providers, 26+ models, 5% fee, Solana-first, x402 protocol) is tight. Launch readers will reinforce the same story across channels.
- **Tone** is honest and direct. No "revolutionary" fluff. Good.
- **Pricing transparency** is strong across all drafts. Explicit 5%, explicit breakdown. This is a rare launch to not hide anything.
- **Escrow as moat** is consistently surfaced. Right call.
- **Code examples** use real package names and commands — no "coming soon" vapor.
- **Staged launch sequence** (registry submissions Day 3 → HN Day 3-4 → X Day 4 → blog Day 5) is well-thought-out.

---

## Priority order for edit pass

1. **Fix escrow address** across both files (5 minutes, blocking).
2. **Fix audit claim** in MCP registry JSON (5 minutes, blocking).
3. **Add Telsi/RustyClaw proof line** to HN body, X thread, blog post (30 minutes, high-impact).
4. **Add x402-rs layer-carving paragraph** to blog + HN predicted comments (20 minutes, defuses biggest diligence risk).
5. **Fix GateRouter typo** in HN + X (1 minute).
6. **Soften the 400 vs 100 RPS claim** (5 minutes).
7. **Security warning on SOLANA_WALLET_KEY** in blog, Cursor, OpenClaw drafts (15 minutes).
8. **Resolve license inconsistency** (5 minutes, research-dependent).
9. **Resolve GitHub org name** (public solveladev vs solvela-ai) across all drafts (10 minutes).
10. **Fill metrics placeholders** in Solana Foundation email (time depends on data).

Total edit time: ~90 minutes if all numbers are ready. Everything else is subjective; these 10 items are the objective fixes.
