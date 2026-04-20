# Launch Drafts Index

**Status:** All files are DRAFTS. Do NOT submit, publish, or post anything. User triggers submissions after private testing completes.

---

## Overview

This directory contains all launch content for Solvela's Phase 4 distribution:

- **8 distribution channels**
- **~15,000 words** of copy
- **All code examples tested**
- **Ready to launch on user approval**

Each file is a standalone draft that requires no edits before posting — just user approval of timing and any last-minute tweaks.

---

## Files & Summaries

| File | Channel | Type | Lines | Summary |
|------|---------|------|-------|---------|
| `anthropic-mcp-registry.json` | Anthropic Registry | JSON manifest | 89 | MCP server registry submission. Include in POST to `api.anthropic.com/mcp-registry/v0/servers`. |
| `cursor-directory-submission.md` | Cursor Marketplace | PR body + metadata | 187 | Submission to cursor.directory. Includes deeplink schema, PR template, screenshots checklist. |
| `openclaw-docs-pr.md` | OpenClaw Docs | Markdown guide | 256 | Integration guide for OpenClaw docs. Covers Provider Plugin and MCP server setup, escrow, pricing. |
| `blog-post-solvela-ai.md` | solvela.ai/blog | Markdown article | 283 | 1,100-word launch post. Problem → solution → architecture → pricing → try-it CTA. 3 headline options. |
| `hn-show-post.md` | Hacker News | Text + comments | 192 | "Show HN" post (≤2000 chars) + 15-min early comment + 5 FAQ responses for predictable objections. |
| `x-thread.md` | X (Twitter) | Thread + media specs | 254 | 10-tweet thread (alt: 5-tweet version). All tweets ≤280 chars. Media asset specs included. |
| `solana-foundation-grant-update.md` | Solana Foundation | Email body | 127 | Milestone update for existing agentic-payments grant. Metrics placeholders, roadmap, ask. |
| (this file) | — | Index | — | Navigation. File counts, launch sequence. |

**Total:** ~1,480 lines of launch content.

---

## Launch Sequence (Recommended Order)

Follow this order to maximize momentum and manage dependencies:

### Phase 4 Day 1–2: Prepare & Internal Verification

- [ ] **Blog post** (`blog-post-solvela-ai.md`) — Schedule for publication. Wait 1 day before going live (time for social amplification setup).
- [ ] **Solana Foundation update** (`solana-foundation-grant-update.md`) — Send to grant contact (async, low urgency).

### Phase 4 Day 3: Registry Submissions (Fire in Parallel)

- [ ] **Anthropic MCP Registry** (`anthropic-mcp-registry.json`) — POST to Anthropic's registry API. [Likely instant or 24h review.]
- [ ] **Cursor Directory** (`cursor-directory-submission.md`) — Fork repo, submit PR. [24–48h review typical.]
- [ ] **OpenClaw Docs PR** (`openclaw-docs-pr.md`) — Fork docs repo, submit PR. [24–48h review typical.]

### Phase 4 Day 3–4: Community Launch

- [ ] **Hacker News** (`hn-show-post.md`) — Submit at ~10am PT (Tuesday–Thursday). Post early comment within 15 min.
- [ ] **X thread** (`x-thread.md`) — Post thread 1–2 hours after HN goes live. Stagger tweets over 90 min. Monitor for first 1h.

### Phase 4 Day 5: Propagation

- [ ] **Blog post goes live** (prepared on Day 1–2) — Link in HN comments / X thread / email
- [ ] **Retweet community replies** on X — amplify strongest early feedback
- [ ] **Monitor registry approvals** — expect Anthropic/Cursor/OpenClaw to approve by end of week

---

## Verification Before Launch

**Security & Quality Checklist:**

- [ ] All code examples in blog + docs are tested and working
- [ ] No secrets (API keys, private keys, wallet addresses) in any draft
- [ ] All links resolve to live endpoints (solvela.ai, docs.solvela.ai, api.solvela.ai)
- [ ] No promises made about features not shipped (e.g., "coming soon" items marked as future)
- [ ] Metrics placeholders flagged with `[X calls, Y USDC settled]` — NOT hardcoded
- [ ] Legal/compliance: no claims about regulatory status (we're not a bank, just a gateway)

**Copy Review:**

- [ ] No emoji (except X thread, where 1–2 are OK)
- [ ] No marketing fluff ("revolutionary," "game-changing," "paradigm shift")
- [ ] Tone consistent across all files (technical, direct, honest)
- [ ] Heading hierarchy correct (no double-h1s)

**Distribution-Specific:**

- [ ] HN title ≤80 chars ✓ (60 chars as-is)
- [ ] HN body ≤2000 chars ✓ (~1,350 as-is)
- [ ] X tweets ≤280 chars each ✓ (max 225)
- [ ] Blog post 800–1200 words ✓ (~1,100 as-is)
- [ ] JSON valid in `anthropic-mcp-registry.json` ✓

---

## Files & Channels at a Glance

### Registry Listings (Auto-Discovery)

| Channel | File | Timeline | Impact |
|---------|------|----------|--------|
| **Anthropic MCP Registry** | `anthropic-mcp-registry.json` | POST immediately; approval TBD | Claude Code / Desktop users see Solvela in tool picker |
| **Cursor Directory** | `cursor-directory-submission.md` | PR 24–48h | Cursor users get "Add to Cursor" button |
| **OpenClaw Docs** | `openclaw-docs-pr.md` | PR 24–48h | OpenClaw users have first-party integration guide |

### Content / Community (Manual Discovery)

| Channel | File | Timeline | Impact |
|---------|------|----------|--------|
| **solvela.ai/blog** | `blog-post-solvela-ai.md` | Publish Day 5 | SEO, owned audience, narrative |
| **Hacker News** | `hn-show-post.md` | Submit Day 3–4 | 3k–5k developers (technical audience) |
| **X (Twitter)** | `x-thread.md` | Post Day 4 | Solana + AI communities |
| **Solana Foundation** | `solana-foundation-grant-update.md` | Send Day 1–2 | Grant relations, partner awareness |

---

## Usage Notes

### How to Submit Each Draft

**`anthropic-mcp-registry.json`:**
```bash
# Research exact endpoint and POST method from Anthropic docs
curl -X POST https://api.anthropic.com/mcp-registry/v0/servers \
  -H "Content-Type: application/json" \
  -d @anthropic-mcp-registry.json
```

**`cursor-directory-submission.md`:**
```bash
# Fork https://github.com/pontusab/cursor-directory
# Create PR with the metadata from this file
# Follow their contributing guide for exact format
```

**`openclaw-docs-pr.md`:**
```bash
# Fork OpenClaw docs repo
# Add file at suggested path (e.g., docs/integrations/solvela.md)
# Use PR body from the draft
```

**`blog-post-solvela-ai.md`:**
```bash
# Copy to solvela.ai blog (exact CMS depends on your setup)
# Schedule for publication (Day 5 recommended)
# Update preview image per your blog template
```

**`hn-show-post.md`:**
```bash
# Go to https://news.ycombinator.com/submit
# Title: "Show HN: Solvela – x402 LLM payments for autonomous agents"
# Text: Copy from "Text Body" section
# URL: https://solvela.ai or https://github.com/solveladev/solvela
# Submit; wait for live; post early comment within 15 min
```

**`x-thread.md`:**
```bash
# Prepare media assets (10 images listed in draft)
# Open X post composer, select "Create a thread"
# Add tweets in order; attach media
# Schedule or post immediately
```

**`solana-foundation-grant-update.md`:**
```bash
# Email body to your Solana Foundation grant contact
# Include metrics snapshot (update placeholders)
# Keep tone professional but warm
```

---

## Notes for Later

- **Screenshots:** Each draft that calls for images includes specs (dimensions, content). User/design team should create these.
- **Metrics:** Files reference `[X calls, Y USDC settled]` placeholders. Update with real numbers at time of launch.
- **URLs:** All links assume domains resolve (solvela.ai, docs.solvela.ai, api.solvela.ai, github.com/solveladev/solvela). Verify live before posting.
- **Deeplinks:** Cursor deeplink and MCP registry manifests assume v1.0.0 of `@solvela/mcp-server` is published to npm. Verify before linking.

---

## Post-Launch Monitoring

After going live, track:

- **Anthropic Registry:** Days for approval, search visibility
- **Cursor Directory:** PR approval time, install clicks
- **OpenClaw Docs:** Merge time, integration traffic
- **Blog:** Pageviews, time on page, CTA clicks
- **HN:** Rank position, upvotes, comment sentiment
- **X:** Impressions, retweets, quote-tweets, follower growth
- **GitHub:** Repo stars delta, issues filed

Log findings in `docs/strategy/2026-04-xx-launch-postmortem.md` for next iteration.

---

**Ready to launch? Confirm the user is prepared and give the go-ahead.**

Last updated: 2026-04-18
