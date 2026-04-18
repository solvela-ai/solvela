# MCP Plugin Plan — Review Report (rev 1.3)

**Plan reviewed:** `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md` (revision 1.3)
**Date:** 2026-04-18
**Reviewers:** `oh-my-claudecode:critic` + `oh-my-claudecode:architect` (parallel), synthesized by orchestrator
**Prior reviews:** `2026-04-18-mcp-plugin-plan-review.md` (rev 1.1), `2026-04-18-mcp-plugin-plan-review-r8-addendum.md` (R8 addendum)
**Verdict:** APPROVE WITH AMENDMENTS — 2 P0 + 4 P1 + several P2 nits. No §4 decisions reopened.

---

## 1. Executive summary

Plan is structurally sound. Hybrid C (cargo-dist + npm `optionalDependencies`) is consistently applied across §4.4, §7 R8, §8, §2.2, §10, and the change log — no stale rev 1.2 "cargo-dist only" text survived in decision-level sections. The 13 amendments from the rev 1.1 review + the R8 addendum were all applied cleanly.

What remains are execution-level gaps that prior revisions missed:

- **P0-1:** `deposit_escrow` tool is approved with success criteria and risk entry but **no T-task anywhere implements it**.
- **P0-2:** §4.5 cumulative escrow cap depends on `~/.solvela/mcp-session.json`, which only gets written in Phase 5 (T5-B) — creating a restart-bypass window from Phase 1 through Phase 4.

Fix those two and rev 1.3 ships.

---

## 2. P0 — fix before execution

### P0-1. `deposit_escrow` has no implementation task

**Evidence:**
- Tool is APPROVED in §4.5 (line 112).
- Success criteria S11 (line 227) and S11.5 (line 228) reference it.
- Risk R7 (line 371) mitigates its misuse.
- `grep T[0-9].*escrow` across the plan: zero implementation hits. Referenced only in decision / success-criteria / risk contexts.

**Consequence:** an executor following Phase 1 → Phase 5 hits S11 verification with no instructions for what to build. Likely result: ad-hoc unreviewed work, or the tool silently gets dropped from V1.

**Action:** Add a T-task (suggested placement: Phase 2 as T2-G, after signing works):

> **T2-G.** Implement `deposit_escrow` MCP tool handler in `sdks/mcp/src/index.ts`. Gate behind `SOLVELA_ESCROW_MODE=enabled` env var. Wire through `buildEscrowDeposit` from `@solvela/sdk`. Enforce `SOLVELA_MAX_ESCROW_DEPOSIT` per-call cap. Implement in-memory cumulative session tracker for `SOLVELA_MAX_ESCROW_SESSION` (T5-B upgrades this to file-persisted). Add tool to `TOOLS` array conditionally on the gate env var.

Composes naturally with P0-2 — the in-memory tracker works pre-T5-B, and T5-B upgrades it to survive restarts.

### P0-2. Cumulative escrow cap is unenforceable before Phase 5

**Evidence:**
- §4.5 line 195: "tracks cumulative deposits against `~/.solvela/mcp-session.json` (cross-reference T5-B)."
- T5-B is Phase 5 (line 340). Nothing in Phase 1–4 creates or writes this file.
- S11.5 verification test (5 × $4 calls against $20 cap) runs within one session; does not cover restart-bypass.

**Consequence:** `SOLVELA_MAX_ESCROW_SESSION` is a security claim that is not backed in Phase 1 through Phase 4. A server restart resets the counter. R7 mitigation describes two-layer caps as if both were live from day 1.

**Action (pick one):**
- **(a, recommended):** pull the persistence writer into Phase 1 alongside `deposit_escrow` (~0.5 days).
- **(b):** explicitly document "cumulative cap is in-memory-only until Phase 5" in §4.5 and R7, and add a restart-bypass test to S11.5 in Phase 5.

Option (a) is preferred — the tool ships with a security claim, and a documented-but-real bypass window is bad optics for a payment-facing tool.

---

## 3. P1 — fix before kickoff

### P1-1. npm scope ownership ambiguous for 5 new packages

§3 line 117 confirms publish rights for `@solvela/mcp-server`. T4-G.4 (line 329) introduces 5 new packages under `@solvela/`:
- `@solvela/cli`
- `@solvela/cli-linux-x64`
- `@solvela/cli-win32-x64`
- `@solvela/cli-darwin-x64`
- `@solvela/cli-darwin-arm64`

If the user owns the `@solvela` npm org scope, this is satisfied. If they own only specific package names, T4-G.5 fails on first publish.

**Action:** Add to §3 Access/credentials:

> `- [ ] **npm org scope @solvela** — confirm user owns the npm org (not just individual package names). Required for T4-G.4 platform packages.`

### P1-2. T4-G release → npm publish automation is missing

T4-G.2 runs `cargo dist init` which generates `.github/workflows/release.yml`. T4-G.4 creates the npm scaffold. T4-G.5 asserts "`@solvela/cli` + platform packages auto-publish to npm." Nothing connects the two: `cargo-dist` knows nothing about npm. No task specifies who copies the built binary into each platform package directory and runs `npm publish`.

**Action:** Insert a sub-task between T4-G.4 and T4-G.5:

> **T4-G.4.5.** Add a GH Actions workflow (or post-job in `release.yml`) that: downloads the four platform binaries from the GH Release, copies each into its respective `@solvela/cli-<platform>` package directory, and runs `npm publish --provenance` for all five packages. Reference biome's `.github/workflows/release_cli.yml` as template.

### P1-3. Phase 4 → Phase 2 implicit ordering

T4-G.6 smoke-tests `solvela mcp install --host=claude-code`. That subcommand only exists after T2-A lands in Phase 2. The plan does not prohibit parallel dev of Phase 2 and Phase 4, so the release pipeline can ship a binary without the `mcp install` subcommand.

**Action (pick one):**
- Gate T4-G behind Phase 2 merge (explicit phase dependency note).
- Split T4-G.6 off from T4-G.1–G.5 — let the pipeline scaffold proceed independently while the smoke test waits for Phase 2.

### P1-4. 402 fixture ownership undefined

T1-I adds a contract test consuming `crates/gateway/tests/fixtures/402-envelope.json`. The sibling AI SDK plan already consumes it (`sdks/ai-sdk-provider/tests/unit/parse-402.test.ts`, `sdks/ai-sdk-provider/tests/unit/adapter-contract.test.ts`). No owner is named in §9.

**Action:** Add one line to §9:

> Gateway team owns `crates/gateway/tests/fixtures/402-envelope.json`. Both consumer plans (MCP, AI SDK) run their contract tests against gateway HEAD in CI. A gateway PR that changes the 402 shape must update the fixture in the same PR.

---

## 4. P2 — nits and watch-during-execution

- **Stale r1.2 parenthetical.** Line 174 Option B description still reads `(r1.2 approved option — single source of truth, no npm UX.)`. The APPROVED header on line 170 is correct, but the bullet is stale. Change to `(r1.2 approved option, superseded by Hybrid C in r1.3)`.
- **Biome template link.** T4-G.4 says "Use biome's `packages/cli/` layout as template" without linking. Add `https://github.com/biomejs/biome/tree/main/packages/%40biomejs/cli` to save the executor 15–30 min of hunting.
- **Missing Linux ARM64 target.** `aarch64-unknown-linux-gnu` absent from T4-G.1 and T4-G.4. Cheap to add now (zero extra `cargo-dist` effort); increasingly common (Graviton CI, Raspberry Pi). Optional — V1.1 is also fine.
- **OIDC provenance scope.** T4-A claims `--provenance` for `@solvela/mcp-server`. T4-G does not state it for `@solvela/cli` or the 4 platform packages. Make explicit or explicitly defer.
- **T2-A Windows-cross-compile guardrail.** The R8 addendum verified `crates/cli/Cargo.toml` is currently clean for Windows cross-compile (no `solana-sdk`, `rustls-tls` only, no `build.rs`). Phase 2 adds `mcp install`. If that work pulls in a dep with native bindings (e.g. `dirs-next` in some configurations), Phase 4 breaks. Add a one-line guardrail to T2-A: "new deps must remain Windows-cross-compile-safe per the R8 addendum audit."
- **T3-C partial-stream failure note.** If `wrapStreamFn` signs + injects + stream drops mid-response, direct-transfer mode has no refund path. Brief note in T3-C's implementation guidance: "escrow mode is the recommended default for Provider Plugin use precisely because direct-transfer partial-stream failures are non-refundable; the escrow claim only fires on a completed response."
- **Hybrid C publish atomicity.** Dangerous failure mode: 3/4 platform packages publish at v1.0.1, 1 fails → `@solvela/cli`'s `optionalDependencies` point at a version that doesn't exist on one platform. Not a plan amendment, but worth a watch-item: ensure the publish pipeline is atomic or has a documented rollback. Biome's workflow handles this — copy the pattern.
- **Sibling plan courtesy note.** The bidirectional note at plan lines 247–248 is adequate. What's missing on the *sibling* plan side: its §9 or §3.8 should, on its next revision, list `@solvela/mcp-server` as a signing-dependency consumer so `@solvela/sdk` restructuring preserves `createPaymentHeader` importability. Not blocking; log in `HANDOFF.md`.

---

## 5. Noted for awareness — no action needed

- **CLI scope (architect F7).** The existing `solvela` CLI is agent-operator-oriented; `mcp install` serves MCP host configurators. A separate `solvela-mcp-install` binary would be conceptually cleaner, but shipping two binaries doubles platform-package complexity for marginal purity. The locked §4.4 decision (fold into existing CLI) is defensible given Hybrid C distribution. No action.
- **Phase sizing.** Phase 4 bump from 4–6d → 5–7d (+4–8h for npm scaffold) is consistent with the R8 addendum's effort delta between Options B and C (6–8h → 12–16h first release). Checks out.
- **wrapStreamFn architecture (architect F8).** Per-request header injection matches the MCP server's 402-retry pattern. The same partial-spend risk exists in both paths; escrow mode is the shared safety net. Architecture is sound.

---

## 6. What was NOT a problem (don't re-litigate)

To avoid wasted edits, these were checked and are fine in rev 1.3:

- T3-D deletion (done cleanly in 1.2).
- T1-A.5 ESM/CJS spike (added in 1.2).
- T1-G → T2-B ordering (called out in 1.2).
- `dev_bypass_payment` warn-only startup check (1.2 decision).
- `SOLVELA_MAX_ESCROW_SESSION` cap *existence* (1.2) — implementation gap is P0-1/P0-2, not the decision itself.
- Package name canonicalization to `@solvela/mcp-server` (1.2 T1-J).
- Phase 0 cross-plan gate (1.2).
- Option B vs Hybrid C trade-off (1.3 locked Hybrid C per user decision; consistent across the document).
- §4.1 auto signing mode, §4.2 MCP + Provider Plugin, §4.3 `@solvela/sdk` dependency, §4.6 full launch slate — all sound.

---

## 7. Consolidated amendment checklist (for rev 1.4)

1. **Phase 2 / §4.5:** Add T2-G — `deposit_escrow` tool handler implementation. (P0-1)
2. **Phase 1 or §4.5:** Pull `~/.solvela/mcp-session.json` persistence into Phase 1, OR document in-memory-only cap until Phase 5 + add restart-bypass test to S11.5. (P0-2)
3. **§3 Access/credentials:** Add `@solvela` npm org scope ownership check. (P1-1)
4. **Phase 4:** Insert T4-G.4.5 — GH Actions npm-publish automation. (P1-2)
5. **Phase 4:** Make T4-G → Phase 2 dependency explicit, or split T4-G.6 from T4-G.1–G.5. (P1-3)
6. **§9:** Add 402 fixture ownership line (gateway team owns; same-PR updates; CI runs contract tests against gateway HEAD). (P1-4)
7. **P2 items (batch):** Fix stale r1.2 parenthetical at line 174; add biome template link in T4-G.4; decide on `aarch64-unknown-linux-gnu`; state OIDC provenance scope for `@solvela/cli`; add Windows-cross-compile guardrail to T2-A; add partial-stream escrow-recommendation note to T3-C.

---

## 8. Evidence index

All references verified during this review:

- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:112` — §4.5 `deposit_escrow` APPROVED.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:174` — stale r1.2 Option B parenthetical.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:195` — cumulative cap references `~/.solvela/mcp-session.json`.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:227-228` — S11 / S11.5 success criteria.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:247-248` — bidirectional coordination note.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:288` — T2-A defines `mcp install` subcommand.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:325-331` — T4-G.1 through T4-G.6.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:331` — T4-G.6 smoke test depends on T2-A.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:340-343` — T5-B in Phase 5.
- `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md:371` — R7 two-layer cap claim.
- `crates/gateway/tests/fixtures/402-envelope.json` — shared fixture, no owner named.
- `sdks/ai-sdk-provider/tests/unit/parse-402.test.ts` — sibling plan already consuming fixture.
- `sdks/ai-sdk-provider/tests/unit/adapter-contract.test.ts` — second sibling consumer.
- `crates/cli/Cargo.toml:1-32` — clean Windows-cross-compile-safe dep tree (R8 addendum baseline).
- `crates/cli/src/main.rs:26-72` — existing CLI subcommand set (wallet/models/chat/stats/health/doctor/loadtest/recover).
- `docs/superpowers/plans/2026-04-16-vercel-ai-sdk-provider-plan.md:46` — sibling out-of-scope excludes `@solvela/mcp-server` edits.
- `docs/superpowers/plans/2026-04-16-vercel-ai-sdk-provider-plan.md:229-253` — sibling §3.8 ESM/CJS strategy.

---

*End of report. Apply amendment checklist (§7) for rev 1.4, then execution is unblocked.*
