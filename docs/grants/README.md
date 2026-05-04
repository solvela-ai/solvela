# Solvela — Grant Strategy

> Working index for grant applications and the rationale behind each. The goal
> is *sustainability*, not growth-at-all-costs: cover infrastructure, fund a
> third-party audit of the on-chain escrow, and accelerate ecosystem
> integrations.

## Strategy in one paragraph

Solvela is the first production-quality Solana-native x402 implementation, with
an Anchor escrow program deployed at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU`
on mainnet. The protocol crates are MIT; the gateway is BSL-1.1 with a
generous Additional Use Grant. That license shape makes Solvela eligible for
public-goods grants (because anyone can fork the protocol), while preserving
acquihire optionality on the gateway. We pursue grants that compound: each one
should produce a permanent, verifiable artifact (a published audit, a merged
upstream PR, a public dashboard) rather than greenfield engineering.

## Tier 1 — apply now (high fit)

| Program | Status | Ask | Tracking file |
|---|---|---|---|
| **Solana Foundation Grants** | Draft ready | $50,000 | [`solana-foundation-application.md`](./solana-foundation-application.md) |
| **Superteam (regional)** | Draft ready | $5,000–$15,000 | [`superteam-application.md`](./superteam-application.md) |

These do not conflict — Solana Foundation is the umbrella program, Superteam is
the regional builder-funded variant. Apply to both. Superteam typically responds
faster (days–weeks) and is a useful proof-point when the Foundation reviews.

## Tier 2 — apply after Tier 1 traction or a specific trigger

| Program | When to apply | Why |
|---|---|---|
| **Colosseum hackathon** | Next cycle window | Solvela is a credible DePIN/Infra or Consumer/Agents track entry. Mid-bracket placements get $5k–$25k + investor intros. Even no-prize entries move the brand. |
| **Helius / Triton / QuickNode partnership grants** | After audit publishes | Solvela drives Solana RPC volume. RPC providers fund integrations that drive their business. Quiet email (`partnerships@helius.dev`) with usage projections. |
| **Squads / Drift / Marginfi BD funds** | When a credible integration target exists | Less "grant," more "integration bounty." A 2-paragraph cold email when Solvela could service their treasuries' AI agents. |
| **Coinbase / Base Ecosystem Fund** | Only after we ship an EVM PaymentVerifier | The `PaymentVerifier` trait is already chain-agnostic by design — but actually shipping an EVM adapter is the unlock. Don't apply pre-implementation. |

## Tier 3 — long shots, low effort

| Program | Notes |
|---|---|
| **a16z crypto CSS** | Heavy application; only worth it if we're seriously raising. |
| **Jump Crypto / Multicoin** | Investments, not grants. A 3-sentence DM to a partner once volume is real costs nothing. |
| **Solana Foundation Speaker Slots (Breakpoint, Accelerate)** | Not money but distribution. The audience is full of acquirers. |

## Hard rules

- **No token, ever.** A token would dissolve the regulatory posture in
  [`docs/product/regulatory-position.md`](../product/regulatory-position.md) and
  scare every plausible acquirer. We will decline grants that require token
  allocation in return.
- **MIT remains permanent on libraries, SDKs, and dashboard.** Grants do not
  change license terms.
- **Public milestones only.** Every grant must be reportable in public PRs,
  on-chain transactions, or a published audit. If the milestone can't be linked
  to from this repo, we don't take the money for it.
- **No greenfield asks.** We ask for funding to *finish, audit, integrate, or
  publish* what already ships. Greenfield grants don't fit Solvela's stage.

## Reusable application packet

Save these once, paste into every application:

1. **Executive one-liner** — "Solana-native x402 reference: deployed Anchor
   escrow at `9neDH…HLU`, MIT protocol crates, live gateway at
   `api.solvela.ai`, 5 LLM providers, 400 RPS p99 < 300 ms."
2. **5-slide deck** — problem, solution, demo screenshot, on-chain proof, ask.
   Stored in `assets/decks/grants-deck.pdf` (TODO).
3. **2-min Loom demo** — agent makes a real USDC payment end-to-end. Stored at
   the link in each application's "Public artifacts" section.
4. **Live metrics page** — `metrics.solvela.ai` (TODO). Until that's built, link
   `https://api.solvela.ai/health` and the Solscan view of the escrow program.
5. **Regulatory posture doc** —
   [`docs/product/regulatory-position.md`](../product/regulatory-position.md).
   This is uniquely valuable; most projects this size haven't done it.
6. **License explainer** — the [Licensing section in
   `README.md`](../../README.md#licensing) plus the [commercial license
   page](https://docs.solvela.ai/enterprise/commercial-license).

## What goes in this directory

- One markdown file per program we apply to. Filename:
  `<program-slug>-application.md`.
- Each file is a working draft, not the submitted text. We track edits here so
  the grant evaluator's response can be reconciled against what we sent.
- After submission, append a `## Submitted` section with date, grant officer
  contact (if any), and the response when it arrives.
- After award or denial, append an `## Outcome` section. Lessons go in the
  *next* application's draft.

## Operator-side blockers (not this repo's problem to solve, but list them)

- Bio paragraph for "Team" sections — Kenneth's actual prior work / employers
  that signal "ships things"
- Audit quote from Neodyme / OtterSec / Halborn before quoting a hard $25k for
  the audit line item
- USPTO trademark filing for SOLVELA (~$350/class × 2 classes)
- GitHub Sponsors profile on `solvela-ai`
- Polar.sh page at `polar.sh/solvela-ai`
- Loom demo recording (2 min, agent making a real USDC payment)
- `metrics.solvela.ai` Vercel deploy

When all of these are ticked off, the application packet is acquihire-grade as
well — same artifacts an acquirer's diligence team would request.
