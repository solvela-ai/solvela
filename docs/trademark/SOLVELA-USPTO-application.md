# SOLVELA — USPTO Trademark Application

> **Status:** Draft, ready for review and submission via [USPTO TEAS Plus](https://www.uspto.gov/trademarks/apply/teas-plus). Not yet filed.
>
> **Audience:** the maintainer (operator-side preparation doc) and any IP counsel reviewing the filing strategy.
>
> **Last refreshed:** 2026-05-04.

---

## Why file

The mark `SOLVELA` is the most ownable single asset in an exit scenario. The codebase is BSL/Apache/MIT split, the domain is registered, the GitHub org exists — but without a registered trademark, an acquirer is buying an unregistered common-law mark with national exposure but no national priority. A USPTO registration converts that into a federally protected, transferable asset for ~$700 of filing fees and ~12 months of prosecution time.

Filing now also locks priority. If anyone files a similar mark in classes 9 or 42 between now and an acquisition, the project's bargaining position weakens.

## Filing strategy at a glance

| Decision | Recommendation | Rationale |
|---|---|---|
| Filing system | **TEAS Plus** | $250/class vs $350/class for TEAS Standard. Requires using the USPTO ID Manual descriptions; we comply below. |
| Basis | **§ 1(a) — actual use in commerce** | The mark is in active use: hosted gateway at `api.solvela.ai`, published packages on crates.io / npm / PyPI, GitHub org. |
| Classes | **9 + 42** | Class 9 covers the downloadable software (CLI, SDKs, daemons). Class 42 covers the hosted SaaS gateway. Both are needed because the project ships both forms. |
| Standard character vs design | **Standard character** | We don't have a finalized logo lockup. Standard character protects the word mark in any styling. We can file a design mark separately later if a logo solidifies. |
| First use date | Earliest verifiable date the mark was used in commerce in connection with each class | See "Specimens" below for evidence. |
| Owner | **The maintainer as an individual** initially; assign to an entity at incorporation | Filing as an individual is simpler and avoids a name change later if no entity exists yet. Assignment to an LLC/Inc post-formation is straightforward. |

## Identification of goods and services

These descriptions are taken from the USPTO ID Manual to qualify for TEAS Plus. Do **not** edit them at filing time without checking the ID Manual — paraphrasing disqualifies TEAS Plus and bumps the fee to $350/class.

### Class 9 — Downloadable software

**Identification text (TEAS Plus compliant):**

> Downloadable computer software for routing and monitoring application programming interface requests; downloadable computer software for verifying blockchain payment transactions; downloadable computer software development kits (SDKs) for use in connection with artificial intelligence agents and large language model providers; downloadable command line interface software for managing application programming interface gateway configuration.

**ID Manual entries this aligns to:**
- "Downloadable computer software for [function]" (G&S ID: 009-0070)
- "Downloadable computer software development kits (SDK)" (G&S ID: 009-0136)

### Class 42 — SaaS

**Identification text (TEAS Plus compliant):**

> Software as a service (SAAS) services featuring software for routing application programming interface requests between artificial intelligence agents and large language model providers; software as a service (SAAS) services featuring software for verifying blockchain payment transactions and settling on-chain payments; providing temporary use of online non-downloadable software for monitoring application programming interface usage and computing platform fees.

**ID Manual entries this aligns to:**
- "Software as a service (SAAS) services featuring software for [function]" (G&S ID: 042-0287)
- "Providing temporary use of on-line non-downloadable software for [function]" (G&S ID: 042-0226)

## Specimens of use

USPTO requires a specimen per class showing the mark in connection with the identified goods/services as actually offered in commerce. Prepare these at filing time:

### Class 9 specimen options

- **Preferred:** screenshot of the `solvela-cli` GitHub release page at `github.com/solvela-ai/solvela/releases` showing `SOLVELA` in the project header, the binary name `solvela`, and a download link for the most recent release.
- **Alternate:** screenshot of `crates.io/crates/solvela-cli` showing the package listing with `SOLVELA` branding.
- **Alternate:** screenshot of `npmjs.com/package/@solvela/sdk` (or equivalent) showing the published package.

### Class 42 specimen options

- **Preferred:** screenshot of the hosted dashboard at `app.solvela.ai` showing the `SOLVELA` mark in the header and a checkout/sign-up flow or a paid endpoint description.
- **Alternate:** screenshot of `solvela.ai` marketing page describing the hosted gateway service with `SOLVELA` in the header and a sign-up call-to-action.
- **Alternate:** screenshot of `docs.solvela.ai/quickstart` showing the hosted endpoint URL `https://api.solvela.ai/...` alongside the `SOLVELA` brand.

Each specimen must be a flattened PNG or JPG, max 5 MB, that clearly shows:
1. The mark `SOLVELA` (or `Solvela` — capitalization is preserved at filing but does not narrow protection for a standard character mark)
2. The goods or services offered
3. A way for the consumer to obtain them (download link, sign-up, hosted endpoint URL)

Avoid specimens that are merely advertising material with no purchase pathway visible — examiners reject those frequently in class 9 specifically.

## First use dates

These need to be established as accurately as possible from public records. The maintainer should verify each before filing.

| Class | "Date of first use anywhere" | "Date of first use in commerce" | Evidence |
|---|---|---|---|
| 9 | First public commit using the SOLVELA name in a published binary or package | First public release on a registry (crates.io / npm / PyPI) using SOLVELA | Git tag dates, registry publication timestamps |
| 42 | First time `api.solvela.ai` answered a request from a third party | Same | Server logs, DNS records, deploy artifacts |

If unsure, file with the most conservative (latest) verifiable date. Overstating first-use dates can render the registration vulnerable to cancellation later; understating costs nothing.

## Owner information

To complete at filing time:

- **Owner name:** [legal name of the maintainer, individual; or entity name if incorporated by filing date]
- **Owner type:** Individual (US citizen) or LLC / Corporation (state of formation)
- **Owner address:** [mailing address; this becomes part of the public record]
- **Citizenship / state of formation:** [US state for individuals; state of incorporation for entities]
- **Email:** kd@sky64.io
- **Domestic representative:** Not required for US-based applicants

## Fees

| Item | Amount |
|---|---|
| TEAS Plus, Class 9 | $250 |
| TEAS Plus, Class 42 | $250 |
| Statement of Use (already filing under §1(a) so no additional SOU fee) | $0 |
| **Total filing fees** | **$500** |

If TEAS Plus disqualified during examination (e.g., a specimen is rejected and the mark must be argued under TEAS Standard), the fee bumps to $700 total. Budget $700 to be safe.

Maintenance fees later:
- Sections 8 + 15 declaration between years 5–6: ~$525/class
- Section 9 renewal at year 10 and every 10 years thereafter: ~$525/class

## Pre-filing search

Before filing, run a knock-out search on the USPTO TESS database and a Google search to surface obvious conflicts. Items to check:

- [ ] TESS search for `SOLVELA` exact match — result: [run before filing]
- [ ] TESS search for `SOLVEL*` wildcard in classes 9 and 42 — result: [run before filing]
- [ ] TESS search for phonetic equivalents (`SOLVELLA`, `SOLVELLA`, `SOLVALA`, `SOLVELLA`, `SOLVELLA`) in classes 9 and 42 — result: [run before filing]
- [ ] Google search for "Solvela" in software / SaaS contexts to find unregistered common-law users with senior priority — result: [run before filing]
- [ ] Domain check: confirm `solvela.ai`, `solvela.com`, `solvela.io`, `solvela.dev` ownership and use status to surface anyone who may already be using the mark commercially under a different TLD

If any conflict surfaces, decide between:
1. Filing anyway and arguing for coexistence based on different goods/services
2. Switching to a related mark (`SOLVELA AI`, `SOLVELA GATEWAY`) that may be distinguishable
3. Pre-filing rebrand if the conflict is clearly a senior user in the same lane

The maintainer should consider a paid clearance search via a service like CompuMark or Corsearch before filing if the knock-out search surfaces anything close. Cost: ~$500–$1,500. Optional, but cheap insurance against a Section 2(d) refusal.

## Prosecution timeline (typical)

| Month from filing | Event |
|---|---|
| 0 | File via TEAS Plus |
| 3–4 | Examining attorney assigned, first office action issued (if any objections) |
| 6 | Office action response due (if any) |
| 7–8 | Approval for publication, if no further objections |
| 9 | Publication in the Trademark Official Gazette; 30-day opposition window opens |
| 10 | Opposition window closes |
| 11–12 | Notice of registration / certificate of registration issued |

Office actions are common — most marks get at least one. Routine office actions (specimen issues, identification clarifications) are typically handled by the maintainer pro se using the TEAS response form. A § 2(d) likelihood-of-confusion refusal benefits from counsel; budget $1,500–$3,000 for an attorney response if that comes back.

## Use after registration

To preserve the mark:

- **Always use the mark as a brand**, not as a generic term. "Solvela" is a brand name; "the gateway" or "the SDK" is the noun. Do not write "use a solvela to verify payments." Write "use the Solvela gateway to verify payments."
- **Use the ® symbol** only after registration issues. Before that, use ™ on the first prominent occurrence per page.
- **Police use of the mark** by third parties. Cease-and-desist any use in class 9 or 42 contexts that creates confusion. License affiliates explicitly if any partner program emerges.
- **Renewal calendar**: enter Sections 8/15 declaration deadline (5–6 years from registration) and Section 9 renewal (10 years) into a calendar with two-month advance reminders.

## Related filings to consider later

- **EU trademark via EUIPO** — if European usage becomes substantial. ~€850 base + €50/class.
- **UK trademark via UKIPO** — separate from EU since Brexit. £170 base + £50/class.
- **Madrid Protocol filing** for international expansion using the US registration as the base — fees scale by country.
- **Design mark** (logo lockup) once a logo is finalized. Filed as a separate application.

These are post-acquisition concerns. The US registration is sufficient for the next 12–24 months of operation and for any plausible exit conversation in that window.

## Operator checklist

- [ ] Confirm legal name and address for the application
- [ ] Decide individual vs entity ownership
- [ ] Run TESS knock-out searches (4 listed above)
- [ ] Run Google common-law search
- [ ] Optional: paid clearance search via CompuMark / Corsearch
- [ ] Verify and record first-use dates from public artifacts
- [ ] Capture specimens (one per class, both prepared in advance)
- [ ] Confirm $500–$700 budget available for filing
- [ ] File via TEAS Plus at [teas.uspto.gov](https://teas.uspto.gov/forms/teasplus)
- [ ] Save filing receipt and serial numbers in operator credential vault
- [ ] Add prosecution checkpoints to calendar (months 3, 6, 9, 12)
- [ ] Add post-registration maintenance dates to calendar (years 5, 10, 20)

## Filing receipt destination

After filing, store:

- Application serial numbers (one per class)
- Filing date
- Filed-as image of the application
- Receipt PDF
- Specimens submitted

…in the operator credential vault under `trademarks/USPTO/SOLVELA/`. Reference the serial numbers in `docs/exit-readiness.md` line 78 once filed and update the checkbox.

---

## Disclaimer

This document is not legal advice. The USPTO trademark process is straightforward enough for a pro se applicant in most cases, but a § 2(d) refusal or a specimen rejection in a non-obvious case justifies retaining counsel. Consider a one-hour consult with a trademark attorney before filing if any of the knock-out searches surface anything resembling a conflict.
