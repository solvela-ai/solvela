# Session handoff — 2026-04-16

## Pick up here after the directory rename

**Initiative:** First external plugin for Solvela — `@solvela/ai-sdk-provider` for the Vercel AI SDK. Serves as the template for subsequent plugin builds (LangChain, drop-in OpenAI shims).

## Current state

- **Plan:** `docs/superpowers/plans/2026-04-16-vercel-ai-sdk-provider-plan.md` — **revision 4.1, 1462 lines, 137 KB, ready for execution.**
- **Research backing:** `docs/superpowers/research/2026-04-16-vercel-ai-sdk-provider-research.md`.
- All 10 Open Decisions resolved with ecosystem-researched verdicts. No pending approvals.

## What the session produced

Plan went through the full delegation pipeline:

1. Research (document-specialist, primary sources)
2. Plan authoring (planner)
3. Round 1 review — architect + critic + security reviewers in parallel
4. Round 1 revision (planner) addressing every Tier 1 + Tier 2 finding
5. Round 1 verification (critic) — 0 regressions, 7 P1 items
6. Round 1 P1 polish (planner)
7. Round 3 — three parallel ecosystem researchers surveyed the wider ecosystem for better answers to the 10 Open Decisions
8. Round 3 revision (planner) applied 3 overturns + 3 caveats + 4 ratifications
9. Round 3 verification (critic) — 0 P0s, 9 stale-language residuals
10. Round 3 janitorial sweep (planner)

## Biggest design decisions locked in

- **`LanguageModelV3`**, not V4. V4 is `ai@7-beta`; all real community providers target V3 on stable `ai@6`.
- **`SolvelaWalletAdapter` interface**, not `OpaquePrivateKey` hybrid. Matches Coinbase x402 + Solana Wallet Adapter + AWS SigV4 + wagmi/viem. No runtime-gating complexity. Reference `createLocalWalletAdapter` ships from a separate `./adapters/local` sub-export — browser bundlers tree-shake it away cleanly.
- **`@solana/kit`-first** for any new signer-core package, if the Phase 1 Turbopack spike fails.
- **`(string & {})` escape hatch** on the generated `SolvelaModelId` union.
- **Docs ownership inverted** — minimal README pointing to a canonical Fumadocs page.

## Immediate next step on resume

Execute the plan's DAG:

```
Phase 1 (package scaffolding) + Phase 5 (model registry codegen) in parallel
  → Phase 6 (error declarations)
  → Phase 2 (provider factory)
  → Phase 3 (fetch wrapper — security-critical)
  → Phase 4 (reference adapter implementation)
  → Phase 7 (unit tests — separate test-engineer agent)
  → Phase 8 (integration tests)
  → Phase 9 (docs: minimal README + canonical Fumadocs MDX)
  → Phase 11 (live devnet smoke test)
  → Phase 10 (npm publish — provenance + signed tag + OIDC)
```

All phase agent assignments and work items are in the plan's §6.

## Gated on user-side setup (§2 of the plan)

Execution will pause until these exist:

- [ ] **npm org `@solvela`** created, user added as owner with publish rights
- [ ] **npm 2FA automation token** for CI publish
- [ ] **GitHub OIDC workflow** configured with `id-token: write` permission (for `npm publish --provenance`)
- [ ] **GPG key** for `git tag -s` (signed tags on release)
- [ ] **GitHub PAT** with `public_repo` scope (for the `vercel/ai` community-provider listing PR)

## Related in-flight work (parallel windows)

- **Docs migration to `docs.solvela.ai`** was in progress when the session ended. Plan Phase 9 currently targets `dashboard/content/docs/sdks/ai-sdk.mdx` (the verified Fumadocs home). If the docs migration completed, update the target path in §3.7 and Phase 9 work items accordingly before executing Phase 9.
- **Directory rename** from RustyClawRouter-era naming to Solvela — the session ended immediately before this rename. Memory directory at `~/.claude/projects/-home-kennethdixon-projects-RustyClawRouter/memory/` may need parallel rename if Claude Code doesn't auto-migrate.

## Plugin roadmap order (user-agreed)

1. **Vercel AI SDK provider** ← in progress, plan complete
2. **LangChain adapter** — next, should apply the template from §13 of the current plan
3. **Drop-in OpenAI SDK shims** — one per language (Py/TS/Go), each small but four of them for parity
4. **Go SDK signing** — separate infrastructure track (plan exists at `docs/superpowers/plans/2026-04-10-go-sdk-signing-support.md`), prerequisite for Go-native plugins

## Process rules in effect (from saved memory `feedback_plugin_build_process.md`)

1. Research first, via specialized skills/docs — no memory-based guessing
2. Plan = 100% coverage (tech, env vars, creds, skills/hooks/MCPs, manual actions, infra)
3. Plan reviewed by specialist reviewer(s) before presentation
4. Specialist agents only — never generalists where a specialist exists
5. All code delegated — main agent plans + reviews, never writes code
6. If a subagent is blocked, surface to user; do not pick up the pen
7. Quality > speed, always

## Known friction from this session to address eventually (not blocking)

- The read-only specialist agents (`document-specialist`, `architect`, `critic`, `security-reviewer`, `scientist`) cannot Write. Any research-to-artifact workflow requires a two-stage dispatch (read-only specialist reports findings in reply → writable specialist persists to disk). User asked to address this eventually — three options documented: (1) formalize a "research → persist" two-stage skill; (2) grant Write to the read-only specialists; (3) accept the two-stage overhead as default.
