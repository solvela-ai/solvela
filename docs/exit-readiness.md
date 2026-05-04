# Exit Readiness

> **Audience:** the maintainer (private operational doc) and, on demand, an inbound prospective acquirer who has signed a mutual NDA. Linked from `dashboard/content/docs/enterprise/sponsorship.mdx` for transparency, but the diligence packet (Section 4) is gated.
>
> **Purpose:** when an inbound email lands ("saw your numbers, want to chat"), the reply should be one URL plus one paragraph. This document is that URL and that paragraph.
>
> **Last refreshed:** 2026-05-04.

---

## 1. The deal we're actually willing to do

We're not optimizing for a unicorn outcome. The maintainer's stated goal is:

> "Keep the lights on. I'm not looking to get rich. An exit that pays for the project's sustainability and gives me a reasonable role for 1–2 years is a good outcome."

Translated to a deal sheet, that means we're a willing seller for any of:

| Deal shape | What it looks like | Realistic range |
|---|---|---|
| **Acquihire / source-buyout** | Buyer takes the codebase, brand, escrow program, hosted infrastructure, and the maintainer for 12–24 months. Solvela continues to operate under the buyer's umbrella, possibly rebranded. | $250k – $2M cash + standard vesting on equity if applicable |
| **Source license + perpetual asset transfer** | Buyer takes the gateway codebase under a perpetual exclusive commercial license, with right of first refusal on a future buyout. Solvela continues independently as the open-source maintainer. | $150k – $750k upfront + ongoing license fee |
| **Strategic minority investment** | Buyer takes ≤ 25% of an entity wrapping Solvela, becomes default acquirer if the project ever sells. Maintainer retains operational control. | $250k – $1M for 10–25% |
| **Ecosystem grant + commercial license bundle** | Foundation or ecosystem fund (Solana Foundation, Helius, Coinbase) writes a grant alongside a commercial license to host. No equity. | $50k – $500k grant + recurring license |

We are **not** willing to:

- Ship a token under any structure. The regulatory posture in [`docs/product/regulatory-position.md`](./product/regulatory-position.md) is the project's most valuable single artifact, and a token destroys it.
- Sell only the trademark / domain to a buyer who will not also take responsibility for the existing user base. The community is part of the asset.
- Sign an NDA that prevents us from continuing to operate the open-source project. Any deal must allow Solvela to keep shipping under at least the BSL → MIT path on the existing Change Date schedule.

## 2. What is on offer

| Asset | Status | Notes |
|---|---|---|
| **Source code** | All 14 crates, dashboard, escrow program, SDKs in 4 languages | Per-crate license split documented; CLA/DCO in place |
| **Trademark "SOLVELA"** | TBD — application drafted in [`docs/trademark/SOLVELA-USPTO-application.md`](./trademark/SOLVELA-USPTO-application.md), not yet filed | Buyer should expect to inherit a pending application or clean registration |
| **Domains** | `solvela.ai`, `solvela.dev`, `solvela.io` (verify which are registered) | Registrar lock + 2FA |
| **Mainnet escrow program** | Deployed at `9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU` | Upgrade authority retained by `B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`; transferable at close |
| **Hosted infrastructure** | Fly.io (gateway), Vercel (frontend/docs), Upstash (Redis), Helius/Triton (RPC) | All on operator's accounts; transferable via account ownership change |
| **Crates.io / npm / PyPI / Go module ownership** | `solvela-ai` org on GitHub; published artifacts under solvela-* namespace | Org transferable at close |
| **GitHub org `solvela-ai`** | Repo `solvela`, branch protection, CI, dependabot, release automation, deploy-staleness watchdog | Transferable |
| **Regulatory position memo** | [`docs/product/regulatory-position.md`](./product/regulatory-position.md) | Drafted for attorney consultation; not legal advice but acquirer-friendly groundwork |
| **Hosted gateway customer base** | TBD — quantified at [`solvela.ai/metrics`](https://solvela.ai/metrics) | Updated quarterly |

## 3. What is *not* on offer (and why)

- **The maintainer's other work** at `sky64.io` — separate project, separate IP.
- **The OpenFang Agent OS** referenced in some maintainer-side tooling — unrelated codebase.
- **Personal contracting / consulting time outside the transition window** — bounded by the deal's transition clause.

## 4. Diligence packet (gated — provided after mutual NDA)

A full diligence packet is available on request after a mutual NDA. It contains:

1. **Financials** — sponsorship history (GitHub Sponsors / Polar.sh), commercial license revenue if any, infra spend by line item, all-time USDC volume routed through the escrow.
2. **Operational** — incident log, security advisories (published + draft), uptime history, CI failure rate, dependabot churn.
3. **Legal** — license compliance audit, CLA/DCO contributor list, trademark filings, copyright headers, third-party-dependency license inventory (`cargo-deny` output).
4. **Technical** — architecture diagrams, Anchor escrow program audit notes, threat model, the contents of [`SECURITY.md`](../SECURITY.md) plus internal addenda.
5. **Regulatory** — the public [`regulatory-position.md`](./product/regulatory-position.md) plus an internal addendum covering items the public version omits (specific counsel consulted, jurisdictions reviewed, FinCEN guidance traced).
6. **Roadmap** — features in flight, deferred work, technical debt explicitly catalogued.
7. **Bus-factor mitigation** — runbooks, deploy procedures, secret rotation procedures, every credential the project holds.

Email **kd@sky64.io** with subject line `[Solvela diligence]` to start the process.

## 5. Pre-acquisition checklist (operator-side, public)

Things we keep current so an inbound deal doesn't take six months to close on our side:

- [x] Per-crate license declarations consistent and machine-verifiable (`Cargo.toml` license fields)
- [x] CLA/DCO enforcement on every PR (`.github/workflows/dco.yml`)
- [x] `LICENSE` (BUSL-1.1) and `LICENSE-MIT` present and accurate
- [x] `regulatory-position.md` current and dated
- [x] `STATUS.md` and `CHANGELOG.md` reflect actual deployed state
- [x] `commercial-license.mdx` published and externally reachable
- [x] `sponsorship.mdx` published and externally reachable
- [x] Public metrics page deployed (`solvela.ai/metrics`)
- [ ] USPTO trademark application filed for SOLVELA (classes 9 + 42) — draft at [`docs/trademark/SOLVELA-USPTO-application.md`](./trademark/SOLVELA-USPTO-application.md)
- [ ] Escrow program upgrade authority either documented or finalized (`solana program set-upgrade-authority --final`) — current upgrade authority `B7reP7rzzYsKwteQqCgwfx76xQmNTL4bQ7yk4tQTxL1A`
- [ ] Public BACKERS.md exists once first sponsor lands
- [ ] Quarterly infra-spend disclosure on `/metrics` (depends on first quarter of sponsorship clearing)
- [ ] All operator credentials catalogued in a single sealed location for transfer (1Password vault or equivalent)
- [ ] One-page deal summary (this document, kept current) ready to share
- [x] Demo video production kit ready (script, shot list, pre-record checklist, b-roll guide) at [`docs/demo/`](./demo/)
- [ ] 90-second demo video recorded, uploaded to YouTube (unlisted), embedded in `README.md`, and attached to a `demo-v1` GitHub release

## 6. Realistic acquirer shortlist

Maintained internally, ordered by likelihood. Not a solicitation — these are organizations whose public statements or product roadmap suggest a credible fit.

1. **Helius / Triton / QuickNode** — RPC providers monetizing AI agent traffic. Solvela drives RPC volume, brands well as a Solana-native acquihire, and the deal can be structured as an ecosystem-investment plus integration license.
2. **Coinbase** — owns the x402 specification. A Solana adapter to their reference implementation is a clean fit. Higher bar to engage; we approach via Coinbase Ventures, not BD.
3. **Solana Foundation ecosystem absorption** — open-source the gateway under their wing as canonical Solana × x402 infrastructure. Smaller check size, larger distribution.
4. **OpenRouter / LiteLLM (BerriAI) / Helicone / Portkey** — LLM gateway players who want the crypto rail without building it. Most likely to be skeptical on regulatory exposure; the regulatory memo is decisive here.
5. **Phantom / Backpack / Solflare** — wallet vendors looking to embed agent payments. Smaller checks, faster timelines.
6. **Helio / Sphere** — adjacent stablecoin-on-Solana plays. Cultural fit; check size depends on their own runway.

We do not pursue acquisition conversations actively. We respond to inbound. Acquirers who insist on outbound-driven processes usually compress timelines in ways that destroy the regulatory work, which is the most valuable thing on the table.

## 7. Process

When inbound arrives:

1. Reply within 48 hours with this document's URL and one paragraph naming a deal shape from Section 1.
2. If the buyer is serious, mutual NDA inside two weeks. The NDA must allow continued operation of the open-source project.
3. Diligence packet (Section 4) shared within five business days of NDA execution.
4. LOI within four weeks of NDA, or we walk.
5. Definitive agreement within ninety days of LOI, or we walk.
6. At close: trademark assignment, domain transfer, GitHub org transfer, registry org transfers, escrow upgrade-authority transfer, hosted-account transfers (Fly.io, Vercel, Upstash, RPC, registrar), credential vault transfer.

## 8. What we will not negotiate

- A non-compete that prevents the maintainer from working on Solana, AI agents, or payment rails. Three months of cool-off is the cap.
- A claw-back on the regulatory-position memo. It is published; it stays published.
- A condition that requires us to disable the public gateway before close. We may agree to a managed-transition period; we will not orphan users.
- An indemnity above the deal value. Standard caps apply.

---

## Contact

**kd@sky64.io** with subject line `[Solvela acquisition]`. We do not engage with brokers on cold outreach. Direct contact only.
