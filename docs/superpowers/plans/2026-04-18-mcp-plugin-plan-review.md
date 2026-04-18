# MCP Plugin Plan — Review Report

**Plan reviewed:** `docs/superpowers/plans/2026-04-18-mcp-plugin-plan.md` (rev 1.1)
**Date:** 2026-04-18
**Reviewers:** `oh-my-claudecode:critic` + `oh-my-claudecode:architect` (parallel), synthesized by orchestrator
**Verdict:** APPROVE WITH AMENDMENTS — 2 execution blockers + 3 flagged-item refinements + several smaller fixes

---

## 1. Executive summary

Plan is structurally sound. Six §4 decisions are sound. Research in §2 is grounded in real code. Phases are appropriately sized.

Two issues will break execution on day 1 if not addressed:
- **B1:** `@solvela/sdk` has no `exports` map, so T1-B's subpath import will throw `ERR_PACKAGE_PATH_NOT_EXPORTED` before ESM/CJS interop is even tested.
- **B2:** `deposit_escrow` cap is per-call only; an adversarial model loop drains the wallet $5 at a time regardless of R7 mitigation.

On your three flagged items, the reviews converge on strong positions for items 1 and 2, and present a genuine trade-off for item 3 that only you can resolve.

---

## 2. The three flagged items — verdicts

### 2.1 Phase 3 T3-D — Gateway `Authorization: Bearer` change

**Recommendation: DELETE T3-D. Make `wrapStreamFn` the primary path in Phase 3.**

**Evidence:**
- `crates/gateway/src/middleware/x402.rs:38` — middleware reads only `payment-signature` header.
- `crates/gateway/src/routes/chat/mod.rs:142-144` — chat handler **re-reads headers directly**, bypassing the middleware extension.
- `crates/gateway/src/routes/proxy.rs:110` — proxy handler also re-reads directly.
- Adding a Bearer alias therefore touches **three code paths**, not one. The middleware-bypass compounds the blast radius.
- x402 is an HTTP-402-based protocol; smuggling payment into `Authorization: Bearer` conflates auth with payment authorization and complicates CLAUDE.md rule 6 (chain-agnostic verifier, future EVM/Base support).

**Alternative already in the plan:** R3 (line 263) lists `wrapStreamFn` as the fallback if `prepareRuntimeAuth` doesn't fire per-call. Promote it to primary: OpenClaw Provider Plugin intercepts the outbound request stream and injects `PAYMENT-SIGNATURE` directly, matching the MCP server's pattern. Zero gateway changes, no cross-plan blocker, no x402 spec smell.

**Action:** delete T3-D, rewrite T3-C to produce a raw `PAYMENT-SIGNATURE` header and inject via `wrapStreamFn`. Remove the §9 gateway-plan coordination bullet.

---

### 2.2 Phase 1 T1-G — `RCR_*` → `SOLVELA_*` deprecation window

**Recommendation: SHORTEN (or drop entirely). CRITICAL: reorder — T1-G must land before T2-B.**

**Evidence that RCR_* is still the primary reader, not legacy:**
- `sdks/mcp/src/index.ts:35-40` — `RCR_SESSION_BUDGET`, `RCR_API_URL`, `RCR_TIMEOUT_MS` are the **primary** reads (not fallbacks).
- `sdks/mcp/README.md:30,50,65-67` — all documentation uses `RCR_*` exclusively.
- `sdks/mcp/src/client.ts:111` — `RCR_API_URL` is a fallback *after* `SOLVELA_API_URL` (partial migration already).
- `sdks/mcp/tests/server.test.ts:50-63` — tests assert `RCR_API_URL` behavior.

**Two problems with "1 minor version" deprecation:**
1. Package is at v0.1.0 with ~0 external users. Maintaining compat machinery has zero upside.
2. **Ordering bug:** §10 appendix config snippets (plan lines 356–420) all write `SOLVELA_*`. If T2-B ships before T1-G, the installer writes `SOLVELA_SESSION_BUDGET` and `index.ts:35` silently ignores it — the installer output is broken.

**Action:**
- Rename `RCR_*` → `SOLVELA_*` in `src/`, `tests/`, `README.md`.
- Accept `RCR_*` silently (no warning) for one patch release; drop at 1.0.0.
- Add explicit phase-dependency note: **T1-G completes before T2-B starts.**

---

### 2.3 §7 R8 — Windows: npm wrapper vs cross-compiled Rust binary

**This is a genuine trade-off. Surfaced for your decision.**

| Path | Pros | Cons |
|---|---|---|
| **A. Keep npm wrapper** (plan as-written, T4-G) | No new CI infra. Familiar `npm install` UX for Node-first users. Ships fast. | Two codebases generating "the same" JSON inevitably drift. T4-G's "shared JSON generators" claim is not architecturally enforceable across Rust↔Node. No parity test is specified. |
| **B. Cross-compile Rust CLI** (replace T4-G) | Single source of truth, no drift risk, single install surface. `crates/cli/Cargo.toml` deps (clap, reqwest, tokio, ed25519-dalek, bs58, sha2, chrono, base64) are all Windows-compatible. | Adds `windows-latest` to CI matrix or `cargo-dist`. ~1 day of release-pipeline work. Users download a ~10MB binary instead of `npm i`. |

**Critic recommended A** (pragmatic, no CI yet).
**Architect recommended B** (eliminates drift class entirely).

**My read:** B pays for itself every release, but A is defensible if your timeline is tight. If you choose A, **add a golden-file parity test** (both CLIs must produce byte-identical JSON for the same inputs) — otherwise T4-G's parity promise is on the honor system.

**Action (pick one):**
- **B:** Replace T4-G with "Add `windows-latest` to CI matrix; publish prebuilt Windows binaries via `cargo-dist` or GitHub Releases."
- **A:** Keep T4-G; add sub-task: "Golden-file parity test covering all four host generators; fails CI on JSON drift."

---

## 3. Execution blockers found outside your three

### B1. ESM/CJS interop will fail on day 1 (near-certain, not "medium" as R1 claims)

**Evidence:**
- `sdks/typescript/package.json:3-4` has `"main": "dist/index.js"` — **no `exports` map**.
- `sdks/mcp/package.json:6` declares `"type": "module"` (ESM).
- `sdks/typescript/src/x402.ts` uses `require()` and `require.resolve()` internally (CJS dynamic loads).

**Consequences:**
- T1-B (`import { createPaymentHeader } from '@solvela/sdk/x402'`) will throw `ERR_PACKAGE_PATH_NOT_EXPORTED` — no `exports` map means subpath imports are disallowed.
- Even if you bare-import the package entry, the internal `require()` calls fail in ESM without `createRequire(import.meta.url)` shims.

**Action:** Add **T1-A.5** as a hard Phase 1 prerequisite:
> "Add `exports` map to `sdks/typescript/package.json` exposing `./x402` and `./types` subpaths. If `@solvela/sdk` must stay CJS, dual-publish or use `createRequire` in the MCP server. Coordinate with `2026-04-16-vercel-ai-sdk-provider-plan.md` §3.8 since it hits the same issue."

Also: re-rate R1 from `medium` likelihood to `high` / `near-certain` and elevate mitigation from "Phase 1 includes interop spike" (hand-waving) to a concrete pre-T1-B task.

---

### B2. `deposit_escrow` cumulative cap missing

§4.5 caps `SOLVELA_MAX_ESCROW_DEPOSIT` at $5 **per call**. Nothing caps total per session or per day. An adversarial model in a loop drains $500 in 100 calls — well inside plausible model misbehavior under prompt injection.

**Action:** Add `SOLVELA_MAX_SESSION_DEPOSIT` (default $20) enforced against `~/.solvela/mcp-session.json` (already planned in T5-B). Add a corresponding success criterion (e.g., S11.5): "`deposit_escrow` refuses when session cumulative deposits would exceed `SOLVELA_MAX_SESSION_DEPOSIT`."

Without this, R7's mitigation is cosmetic.

---

## 4. Non-blocking issues worth fixing before kickoff

### N1. S10 (concurrency-safe budget) placed in wrong phase

S10 (line 206) is listed as a V1 success criterion, but the mutex wrapping `sessionSpent` is T5-A in **Phase 5**. Ship flow today at `client.ts:146` is a non-atomic read-check-then-write. Two parallel `chat` calls under a $0.10 budget with $0.08 costs each can both pass the check and both commit.

**Action (pick one):**
- Promote T5-A into Phase 1 (it's ~10 lines of `async-mutex`).
- Drop S10 from V1 criteria; document as known issue for 1.0.x.

### N2. Package name mismatch

Plan line 6 and npm references use `@solvela/mcp-server`. `sdks/mcp/README.md:10` says `@solvela/mcp`. Pick one; note the rename prominently. (If 0.1.0 was published as `@solvela/mcp`, the 1.0.0 rebrand to `@solvela/mcp-server` is a new-package publish, not an upgrade — npm users need a migration note.)

### N3. S9 unverifiable as written

S9: "`wallet_status` reports real Solana RPC health when `SOLANA_RPC_URL` is set." But `index.ts:126-127` (today) calls `client.health()` which hits the **gateway's** `/health`, not the Solana RPC. Either add a code-change task to `wallet_status` or soften S9.

### N4. R5 contract-test fixture is inert

`crates/gateway/tests/fixtures/402-envelope.json` exists but no test consumes it. R5 claims "contract test catches drift" — currently untrue. Phase 1 should add a real assertion that the gateway's 402 responses match the fixture shape.

### N5. `dev_bypass_payment` blind spot

`crates/gateway/src/routes/chat/mod.rs:146` silently skips payment when the gateway runs with `dev_bypass_payment = true`. A user installing MCP against such a gateway believes they are paying; the gateway silently skips verification. T1-D's startup check covers missing wallet keys, not this mode.

**Action:** Add startup-time assertion that the gateway is not in `dev_bypass_payment` mode (or document as explicit known risk in README).

### N6. Cross-plan dependency graph not enforced

§9 says "wait for sibling plan's Phase 1 spike result before locking MCP's module strategy," but no phase dependency is documented. Without this, both plans can start Phase 1 in parallel and duplicate the ESM/CJS work (or, worse, land contradictory fixes).

**Action:** Add a visible Phase 0 note: "Gated on Vercel AI SDK plan §3.8 extraction decision or `@solvela/sdk` `exports` map PR (T1-A.5), whichever lands first."

---

## 5. Consolidated amendment checklist

Numbered for easy reference when editing the plan:

1. **§4.1 / Phase 3:** Delete T3-D. Rewrite T3-C to inject `PAYMENT-SIGNATURE` via `wrapStreamFn` instead of Bearer. Remove §9 bullet about gateway coordination plan.
2. **Phase 1:** Add T1-A.5: add `exports` map to `sdks/typescript/package.json` + ESM/CJS spike before T1-B.
3. **Phase 1:** Explicit ordering — T1-G completes before T2-B starts.
4. **§4.4 / R8 / T4-G:** Choose path A (golden-file parity test) or B (cross-compile Rust for Windows).
5. **Phase 5 / §4.5:** Add `SOLVELA_MAX_SESSION_DEPOSIT` cumulative cap + S11.5 criterion.
6. **Phase 1 / S10:** Promote T5-A budget-mutex to Phase 1, **or** drop S10 from V1 criteria.
7. **Section 1 / README:** Resolve `@solvela/mcp` vs `@solvela/mcp-server` naming; add migration note if renaming.
8. **Phase 1:** Either add a code change making S9 true (real Solana RPC probe in `wallet_status`) or soften S9.
9. **Phase 1:** Add real contract test consuming `crates/gateway/tests/fixtures/402-envelope.json` (R5 mitigation is currently fictional).
10. **Phase 1 / T1-D:** Add startup check asserting gateway is not in `dev_bypass_payment` mode, or document as known risk.
11. **§7 R1:** Re-rate likelihood to `high` / `near-certain`. Reference T1-A.5 as mitigation.
12. **§9:** Add visible Phase 0 gate on `@solvela/sdk` `exports` map / ESM/CJS spike (cross-plan coordination).
13. **T1-G wording:** "Accept `RCR_*` silently for one patch release; drop at 1.0.0" (not "1 minor version deprecation warning").

---

## 6. Evidence index

File:line references used in this review (all verified):

- `crates/gateway/src/middleware/x402.rs:38` — only `payment-signature` header extracted.
- `crates/gateway/src/routes/chat/mod.rs:142-144` — direct header re-read, bypasses middleware.
- `crates/gateway/src/routes/chat/mod.rs:146` — `dev_bypass_payment` silent bypass.
- `crates/gateway/src/routes/proxy.rs:110` — direct header re-read, bypasses middleware.
- `crates/gateway/tests/fixtures/402-envelope.json` — static fixture, no consuming test.
- `crates/cli/Cargo.toml:11-28` — Windows-cross-compile-safe deps.
- `sdks/typescript/package.json:3-4` — `main` only, no `exports` map (B1).
- `sdks/typescript/src/x402.ts:44-48` — escrow-preferred signing logic (safe to reuse).
- `sdks/mcp/package.json:6` — `"type": "module"` (ESM, conflicts with SDK CJS).
- `sdks/mcp/src/client.ts:111` — `RCR_API_URL` still wired as fallback.
- `sdks/mcp/src/client.ts:146` — non-atomic budget check (S10 gap).
- `sdks/mcp/src/client.ts:242` — `STUB_BASE64_TX` (the thing we're fixing).
- `sdks/mcp/src/index.ts:35-40` — `RCR_*` primary reads.
- `sdks/mcp/README.md:10,30,50,65-67` — `@solvela/mcp` name + `RCR_*` docs.
- `sdks/mcp/tests/server.test.ts:50-63` — tests still use `RCR_API_URL`.

---

## 7. What was NOT a problem

To avoid wasted edits:
- §4.1 (auto signing mode) — sound; matches `x402.ts:45-48` preference.
- §4.2 (MCP + Provider Plugin) — sound; architect agrees Provider Plugin is the OpenClaw differentiator.
- §4.3 (depend on `@solvela/sdk` for v1.0) — sound; MCP public API is the tool list, swap is internal.
- §4.6 (full launch slate) — sound.
- Phase sizing (1–2d / 3–5d / 5–7d / 3–5d / 2–3d) — reasonable.
- Success criteria S1–S8, S11–S15 — verifiable as written.
- Security mitigations: key-zeroization (§2.1), secret redaction (global CLAUDE.md rule), env-var-only secrets (§2.2).

---

*End of report.*
