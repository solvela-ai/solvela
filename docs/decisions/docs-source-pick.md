# Decision: Pick One Docs Source

_Status: recommendation, awaiting user sign-off. Do not delete the unpicked source until approved._

_Last reviewed: 2026-04-30._

> **Note on location:** the original task asked for this file at `.claude/decisions/docs-source-pick.md`. The agent harness blocks writes under `.claude/` paths, so the doc was placed at `docs/decisions/` instead. Move it after sign-off if you want it to live alongside other Claude scratch material.

## Summary

**Recommendation: keep `dashboard/content/docs/` (Fumadocs MDX). Retire `docs/book/` (mdBook).**

Both sources cover roughly the same surface area, but only one is actually deployed and discoverable to users today. The unpicked source has drifted into a parallel-but-stale lane and is a maintenance tax with no payoff.

## Sources Compared

### `docs/book/` -- mdBook (Rust ecosystem static site)

- **Path:** `docs/book/src/SUMMARY.md` plus 28 `.md` files under `getting-started/`, `concepts/`, `api/`, `sdks/`, `operations/`.
- **Build:** `mdbook` with `mdbook-mermaid` and `mdbook-admonish` preprocessors. Build dir: `target/book/`.
- **Coverage:** five top-level sections (Introduction, Getting Started, Core Concepts, API Reference, SDK Guides, Operations).
- **Deployment status:** **not deployed.** No GitHub Actions workflow builds or publishes mdBook. No `target/book/` is checked in. No CDN, Vercel, or GitHub Pages hookup found.
- **Discoverability:** **zero external links.** README.md, STATUS.md, CHANGELOG.md, CLAUDE.md and the dashboard codebase contain no references to `docs/book/`.

### `dashboard/content/docs/` -- Fumadocs MDX (Next.js app)

- **Path:** `dashboard/content/docs/` -- `index.mdx`, `quickstart.mdx`, plus directories `concepts/` (7), `enterprise/` (7), `api/` (6), `sdks/` (7). 29 `.mdx` files total.
- **Build:** Fumadocs MDX, integrated into the Next.js dashboard (`next build`). Source served by `dashboard/src/app/docs/[[...slug]]/page.tsx`.
- **Coverage:** five top-level sections including a full **Enterprise** section (orgs, teams, api-keys, audit, budgets, analytics) that mdBook does not cover.
- **Deployment status:** **deployed** to `docs.solvela.ai` via Vercel subdomain middleware (per `STATUS.md` "Dashboard + Docs" line and "Deployed" table).
- **Discoverability:** referenced from `STATUS.md`, the dashboard's landing page (`config.ts`, `landing-chrome.tsx`), the dashboard's `proxy.ts` and `sitemap.ts`, and one MDX file. This is the URL that lives on the production website.

## Why Fumadocs Wins

| Criterion | mdBook | Fumadocs MDX |
|---|---|---|
| Files | 28 `.md` | 29 `.mdx` |
| Enterprise org / team / budget docs | absent | present |
| Built by CI | no | yes (via `next build`) |
| Public URL | none | `docs.solvela.ai` |
| Linked from README / STATUS | no | yes |
| Interactive components, search, theming | no | yes (Fumadocs ships search, MDX components, themed layout) |

The Fumadocs source is the one users actually read. The mdBook source is shadow content -- if it drifts (and it has, given the missing Enterprise section), nobody notices because nobody hits it.

## Risks of Keeping Both

1. **Documentation drift.** Edits land in one source and not the other; readers see contradictory guidance depending on which URL they happen to find.
2. **Search noise.** If anyone ever wires mdBook up to a build, the org now has two competing canonical docs sites for the same brand.
3. **Maintenance tax.** Every doc change is a 2x edit if we pretend both exist; in practice we update one and the other rots.
4. **Onboarding confusion.** New contributors don't know which one to update for a given change.

## Proposed Follow-Up (separate task, needs user approval)

1. Verify nothing in CI or operator runbooks depends on `docs/book/` (this audit found none).
2. Cherry-pick any prose unique to mdBook into the Fumadocs tree -- spot-check `operations/troubleshooting.md`, `operations/monitoring.md`, and `concepts/how-it-works.md` since Fumadocs has no `operations/` section.
3. Delete `docs/book/` and remove `mdbook` / `mdbook-mermaid` / `mdbook-admonish` from any tooling docs.
4. Update `docs/AGENTS.md` to point at `dashboard/content/docs/` as the single source of truth.

## Counter-arguments Considered

- **"mdBook is more durable / readable as raw markdown."** True, but durable raw markdown is what `dashboard/content/docs/*.mdx` already is -- Fumadocs MDX renders cleanly on GitHub. The "Rust-shop standard" argument doesn't outweigh "this one is actually live."
- **"What if we want a doc site decoupled from the Next.js app?"** The decoupling already happens via subdomain (`docs.solvela.ai`). The Next.js app is the rendering engine, not the URL. Swapping engines later is a small, well-scoped migration; running two engines today is not.

## Decision Owner

User (kd@sky64.io). This file captures the recommendation; the cleanup PR should not land until the user signs off.
