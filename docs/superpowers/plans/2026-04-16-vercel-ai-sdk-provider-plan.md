# Vercel AI SDK Provider for Solvela — Implementation Plan

**Date:** 2026-04-16
**Author:** Planner (Solvela)
**Revision:** 4.1 (Round 3 residual-reference janitorial sweep — stale references to the deleted `OpaquePrivateKey` + runtime-gate design updated to match the adopted `SolvelaWalletAdapter` interface; no structural or substantive changes. See Appendix B for the Round 3 change map; Appendix A retains Round 1 + Round 2 history.)
**Target package:** `@solvela/ai-sdk-provider` (proposed; see Open Decisions §3)
**Research source (authoritative, cited throughout):** `/home/kennethdixon/projects/Solvela/docs/superpowers/research/2026-04-16-vercel-ai-sdk-provider-research.md`
**Status:** Draft pending user approval of Open Decisions (§3).

---

## 1. Overview

### Goal

Ship the first external software plugin for Solvela: a Vercel AI SDK provider package that lets Next.js and TypeScript agent developers use Solvela's payment-gated gateway as a drop-in LLM provider. The package exposes `createSolvelaProvider(config)` which returns a factory `solvela(modelId)` that can be passed to any Vercel AI SDK top-level function (`generateText`, `streamText`, `generateObject`, `streamObject`). The 402 -> sign -> retry x402 handshake is handled transparently via a custom `fetch` wrapper injected into `createOpenAICompatible`, so consumers never see a 402 and never hand-craft a `PAYMENT-SIGNATURE` header [research §4.1]. This package is the template for subsequent integrations (LangChain, drop-in OpenAI shims beyond the existing `@solvela/sdk`, etc.) — see §13 for template-extraction guidance.

### Success criteria (concrete, testable, user-facing)

| # | Criterion | How verified |
|---|---|---|
| S1 | `npm install @solvela/ai-sdk-provider ai @ai-sdk/provider-utils @ai-sdk/openai-compatible` produces a working install under Node >= 18 with ESM [research §3 "ESM-only, Node version"]. | CI `npm install` on clean Node 18 & 20 matrices. |
| S2 | `generateText({ model: solvela('claude-sonnet-4-5'), prompt: '...' })` against a mocked gateway returning 402 then 200 completes end-to-end with a signed retry. | Integration test §7, scenario IT-1. |
| S3 | `streamText({ model: solvela('gpt-4o'), prompt: '...' })` against a mocked 402-then-SSE gateway streams `text-delta` parts with `stream-start` first and `finish` last [research §1 Stream Parts]. | Integration test §7, scenario IT-3. |
| S4 | `generateObject({ model: solvela('gpt-4o'), schema })` returns parsed JSON matching the schema. | Integration test §7, scenario IT-5. |
| S5 | Tool calls (single function, streaming) produce `tool-input-start` -> `tool-input-delta` -> `tool-input-end` -> `tool-call` parts in order [research §1 Stream Parts, §4 "Tool-Calling JSON Shape"]. | Integration test §7, scenario IT-4. |
| S6 | A failing signer callback throws `SolvelaPaymentError extends APICallError` (not a generic `Error`) and is retryable-false. [research §1 "Error Handling Contract"]. | Unit test §7 Unit-E1. |
| S7 | A non-HTTPS `baseURL` outside of test mode throws at construction time. | Unit test §8 Sec-1. |
| S8 | Private key bytes and `PAYMENT-SIGNATURE` values never appear in logs, error messages, `APICallError.responseBody`, `APICallError.requestBodyValues`, `err.stack`, or `err.cause`. | Security review §8, verified by `pr-review-toolkit:silent-failure-hunter` with sentinel-fixture unit tests. |
| S9 | `tsc --noEmit` clean on the package; `tsup` builds `.js` + `.d.ts` for ESM. | CI build step. |
| S10 | Package has a documented migration path from `openai` SDK; the doc includes a working code sample. | README §6, Phase 9. |
| S11 | Coverage: >= 85% line, >= 80% branch on `src/**` excluding `index.ts` re-exports. | `vitest --coverage` report from Phase 7. |
| S12 | Two concurrent `generateText` invocations with a `sessionBudget` that only affords one request produce exactly one signed transaction and one `SolvelaBudgetExceededError` — never both succeed, never both fail mid-flight. | Unit test Unit-6 race scenario (Phase 7). |
| S13 | Hallway test: one external tester follows the README cold and reaches a successful `generateText` call without asking questions; first-sticky-point reported back for Phase 9 iteration. | Phase 9 done criterion. |

### Out of scope (explicit)

- LangChain / LangGraph / LlamaIndex providers. Separate plans.
- Python AI SDK equivalents (LangChain-Python, DSPy). Separate plans.
- Go / Rust AI SDK equivalents. Separate plans.
- Changes to the Solvela gateway itself (routes, middleware, 402 body shape). The provider consumes the existing contract (verified via the fixture introduced in Phase 1; see T1-B fix). Gateway-side changes are separate plans.
- EVM / Base payment paths. Solana-first per CLAUDE.md rule 6.
- Embeddings, image generation, transcription, speech. V1 ships `LanguageModel` only; `createSolvelaProvider(...).textEmbeddingModel(...)` etc. throw `UnsupportedFunctionalityError` (typed, imported from `@ai-sdk/provider`) to communicate intent clearly.
- Multimodal (image URLs, base64 image parts). V1 ships chat text + tool calls only. `supportsUrls` returns an empty record. If ever added, two integration tests (URL passthrough, inline base64) are required.
- `@vercel/ai` middleware registration or Vercel Marketplace listing. Separate plan once this package publishes.
- Edits to the existing `@solvela/sdk` (`sdks/typescript/`) or `@solvela/mcp-server` (`sdks/mcp/`). **Exception:** if the Phase 1 ESM/CJS interop spike (T1-D) fails, §3.8 (new Open Decision) may require republishing `@solvela/sdk` as dual ESM/CJS, extracting signing into a new `@solvela/signer-core`, or inlining signing into this package.
- In-tree browser signer implementations — adapter authors are responsible for ensuring their implementation is safe for the runtimes they target. `LocalWalletAdapter` from `./adapters/local` is explicitly Node/dev-only and documented as such.

---

## 2. What I need from you (consolidated)

Complete before execution begins. Numbered items map to sections below.

- [ ] **npm org ownership** — confirm the `@solvela` npm scope exists and the user has publish rights. If not, create org and grant access. (Risk §11 R5, Open Decision §3.2.)
- [ ] **npm publish token** — create an **automation token with 2FA required** scoped to `@solvela/ai-sdk-provider`. Store in 1Password + GitHub Actions secret `NPM_TOKEN`. Used from CI only, never from a developer laptop. (T2-J fix.)
- [ ] **GitHub Actions OIDC permission** — confirm CI can assume `id-token: write` for `npm publish --provenance`. (T2-J fix.)
- [ ] **GPG signing key for tags** — confirm the publisher has a GPG key configured on GitHub so `git tag -s` works. (T2-J fix.)
- [ ] **GitHub PAT for AI SDK community-provider PR** — standard GitHub PAT with `repo` scope for fork/PR in Phase 10.
- [ ] **Decision on §3.1** — target `LanguageModelV3` (ai v6 stable) vs `LanguageModelV4` (ai v7 beta). **Planner recommends V3 (Round 3 reversal — see §3.1 rationale).** Upgrade path: bump to V4 in the same release that bumps `ai` peer to `^7.0.0`.
- [ ] **Decision on §3.2** — `@solvela/ai-sdk-provider` (scoped) vs `ai-sdk-provider-solvela` (unscoped). **Planner recommends scoped.**
- [ ] **Decision on §3.3** — package location: `sdks/ai-sdk-provider/` in-repo vs separate repo. **Planner recommends in-repo.**
- [ ] **Decision on §3.4** — signing callback API shape. **Planner recommends adapter-interface pattern (`SolvelaWalletAdapter`) — Round 3 reversal; see §3.4.** Reference `createLocalWalletAdapter` ships from a separate entry point (`@solvela/ai-sdk-provider/adapters/local`) marked dev/test only.
- [ ] **Decision on §3.5** — initial model coverage. **Planner recommends full registry codegen from `config/models.toml`.**
- [ ] **Decision on §3.6** — streaming retry in v1. **Planner recommends yes.**
- [ ] **Decision on §3.7** — docs destination (pending docs-site migration). **Planner recommends: author MDX at `dashboard/content/docs/sdks/ai-sdk.mdx` today; docs-site migration picks it up separately. README is the single source of truth.**
- [ ] **Decision on §3.8 (NEW — T1-D)** — ESM/CJS interop strategy if Phase 1 spike fails. **Planner recommends Option B (extract `@solvela/signer-core`) if spike fails; Option A (dual-publish `@solvela/sdk`) as fallback.**
- [ ] **Decision on §3.9 (NEW — T2-L)** — `@solana/web3.js` 1.x vs `@solana/kit` for default-signer adapter. **Planner recommends: route default signer through `@solvela/sdk` (locks to web3.js 1.x for v1.0); add §3.9 revisit trigger when `@solvela/sdk` migrates to `@solana/kit`.**
- [ ] **Decision on §3.10 (NEW — T3 polish)** — accept legacy `RCR_` env prefix? **Planner recommends no — Solvela-native package should not inherit deprecated naming.**
- [ ] **Test wallet** — funded Solana devnet keypair for the optional live-gateway smoke test in Phase 11. Must contain at least 0.10 devnet USDC and enough SOL for rent. Secret stays with user; plan executor never reads it.
- [ ] **Access to `vercel/ai` monorepo fork** — for Phase 10 community-provider PR. User confirms either they submit, or grants the agent permission to use their GitHub PAT.

---

## 3. Open decisions requiring approval

Each decision lists options, the planner's recommendation, rationale (research-cited), and the impact of changing it after shipping.

### 3.1 Target interface version — `LanguageModelV3` vs `LanguageModelV4`

| Option | Target | Pros | Cons |
|---|---|---|---|
| A | `specificationVersion: 'v3'` (`@ai-sdk/provider` 3.x, `ai` 6.x stable) | Ships against stable `ai@6.x` today; matches the channel that real consumers are on; matches the official AI SDK custom-provider docs page (V3-only examples); matches every shipped community provider (OpenRouter `@openrouter/ai-sdk-provider`, Zhipu, etc.). | Will need a literal+peer bump when `ai@7` stable lands. |
| B | `specificationVersion: 'v4'` (`@ai-sdk/provider` 4.0.0-beta.12, `ai` 7-pre) | Matches `@ai-sdk/openai-compatible` main branch; V3 and V4 are structurally identical so `asLanguageModelV4` up-converts seamlessly [research §5 "V3 vs V4"]. | Peer-depends on pre-release `ai@7` (currently `7.0.0-beta.92`, no announced stable timeline). Every consumer on stable `ai@6` would see peer-dep warnings. |
| C | Ship both: dual peer-dep range `"ai": ">=6.0.0"`, runtime-detect provider-utils version and emit `'v3'` or `'v4'`. | Maximum compatibility. | Complex; two test matrices; not idiomatic. |

**Recommendation: Option A (`LanguageModelV3`) — Round 3 reversal.** Rationale (Round 3 ecosystem research): (i) every shipped community provider that the AI SDK docs reference targets V3 — OpenRouter's `@openrouter/ai-sdk-provider` and Zhipu AI's provider both target V3; (ii) the official AI SDK custom-provider documentation page shows only V3 examples; (iii) `ai@7` is at `7.0.0-beta.92` with no announced stable timeline, and shipping V4 would generate peer-dep warnings for every user on stable `ai@6`; (iv) V3 and V4 are structurally identical for the openai-compatible chat surface this provider exposes — migrating later is a single-line change in the same PR that bumps peer to `ai@^7.0.0`. Confidence: high. The forward-looking V4 analysis in the research file is preserved as the upgrade target, not as the v0.1 ship target.

**Upgrade path:** bump to V4 in the same release that bumps `ai` peer to `^7.0.0`. Single-line change to the `specificationVersion` literal plus peer-dep range; no architectural rework. Tracked in §9 Versioning policy as a 0.2 trigger.

**Impact if changed later:** low. Flipping the literal and peer-dep range is a 2-line change.

### 3.2 Package name — `@solvela/ai-sdk-provider` vs `ai-sdk-provider-solvela`

| Option | Name | Pros | Cons |
|---|---|---|---|
| A | `@solvela/ai-sdk-provider` (scoped) | Consistent with `@solvela/sdk` and `@solvela/mcp-server`. Clear ownership signal. | Requires `@solvela` npm org. |
| B | `ai-sdk-provider-solvela` (unscoped) | Matches community convention (e.g., `ai-sdk-provider-claude-code`) [research §3]. | Inconsistent with existing `@solvela/*` packages. Harder to squat-protect. |

**Recommendation: Option A (`@solvela/ai-sdk-provider`).** Consistency with existing `@solvela/*`; both patterns are idiomatic so consistency wins the tiebreaker.

**Impact if changed later:** moderate. An npm rename forces consumer import updates.

### 3.3 Repo location — in-repo `sdks/ai-sdk-provider/` vs separate repo

| Option | Location | Pros | Cons |
|---|---|---|---|
| A | `/home/kennethdixon/projects/Solvela/sdks/ai-sdk-provider/` | Matches `sdks/typescript/`, `sdks/mcp/`, `sdks/python/`, `sdks/go/`. Single PR for gateway + provider changes. Single CI. | Larger monorepo checkout for SDK consumers cloning examples. |
| B | New repo `solvela-ai-sdk-provider` | Lean standalone repo; easier community contribution; independent release cadence. | Breaks `sdks/` convention; cross-repo sync burden for gateway contract changes. |

**Recommendation: Option A (in-repo) for v0.1 launch.** Five other SDKs already live under `sdks/`; gateway contract changes can be paired with SDK updates in one PR.

**Revisit at v1.0 (Round 3 caveat):** industry precedent (Stripe per-language repos, Supabase JS client standalone, every Vercel community provider as a separate repo) favors a separate repo once the SDK needs an independent release cadence against Vercel AI SDK beta cycles. The v1.0 milestone (see §9 Versioning policy) includes a repo-split decision review; if separate-cadence pressure has materialized by then, `git subtree split` extracts `sdks/ai-sdk-provider/` with full history preserved.

**Impact if changed later:** high. `git subtree split` preserves history.

### 3.4 Signing API design — adapter interface (Round 3 reversal)

| Option | Design | Pros | Cons |
|---|---|---|---|
| A | `signPayment: (args) => Promise<string>` callback only — user supplies callback returning a base64 payload. | Provider never touches the private key. | Bare callback has no `label`/identity surface; no canonical "what is the signer" object for logs/metrics; doesn't match the typed-adapter pattern used by Coinbase x402, Solana Wallet Adapter, AWS SigV4, wagmi/viem. |
| B | **`wallet: SolvelaWalletAdapter` adapter interface (recommended).** Required typed adapter. Reference `createLocalWalletAdapter(keypair)` ships from a separate entry point (`@solvela/ai-sdk-provider/adapters/local`) marked dev/test only. | Matches the typed-adapter pattern used by every payment/wallet ecosystem the user's customers already know (Coinbase x402, Solana Wallet Adapter, AWS SigV4, wagmi/viem). Eliminates `OpaquePrivateKey` as a novel abstraction. Removes runtime-gating complexity (production bundles importing only the main entry physically cannot pull local-keypair code through tree-shaking). Hardware-wallet / MPC / remote-signer support is the obvious default rather than a wedged-in afterthought. | Slightly more code to write for a "just give me a quick demo" user — mitigated by shipping `createLocalWalletAdapter` reference. |
| C | Old hybrid: `signPayment` callback (primary) + `OpaquePrivateKey` `wallet:` convenience (Node-only) with runtime-gating. | Familiar from Round 2 plan. | Introduces two novel abstractions (`OpaquePrivateKey`, runtime-gate) where the four reference ecosystems above use one (typed adapter). Requires bespoke runtime-gating logic. `OpaquePrivateKey` doesn't generalize — every framework would re-invent it. |

**Recommendation: Option B (adapter interface) — Round 3 reversal.** Rationale: four independent ecosystems (Coinbase x402, Solana Wallet Adapter, AWS SigV4, wagmi/viem) converge on "typed adapter interface; never accept raw key bytes in SDK constructor." The adapter pattern is idiomatic to every ecosystem the user's customers already know. Eliminates `OpaquePrivateKey` as a novel abstraction, removes runtime-gating complexity, and makes hardware-wallet/MPC/remote-signer support obvious by default rather than wedged in.

**Public adapter interface:**

```typescript
// src/wallet-adapter.ts (public)
export interface SolvelaWalletAdapter {
  /** Adapter identity for logs/metrics. e.g. "local-test-keypair", "phantom", "ledger". */
  readonly label: string;
  /** Sign a parsed 402 payment-required and return the base64 PAYMENT-SIGNATURE header value. */
  signPayment(args: {
    paymentRequired: PaymentRequired402;
    resourceUrl: string;
    requestBody: string;
    signal?: AbortSignal;
  }): Promise<string>;
}
```

**Provider settings shape:**

```typescript
export interface SolvelaProviderSettings {
  baseURL?: string;
  /** Optional scope token for per-wallet API key tagging. NOT a signing key.
   *  Forwarded as a header to the gateway; never used to derive signatures. */
  apiKey?: string;
  /** REQUIRED. Adapter implementing SolvelaWalletAdapter. No escape hatch — every
   *  signer is an adapter; raw private keys are not accepted at this boundary. */
  wallet: SolvelaWalletAdapter;
  headers?: Record<string, string>;
  sessionBudget?: bigint;
  maxBodyBytes?: number;
  allowInsecureBaseURL?: boolean;
  fetch?: FetchFunction; // override for testing
}
```

**Reference implementation entry point.** `createLocalWalletAdapter(keypair: Keypair): SolvelaWalletAdapter` ships from a **separate package entry point** (`@solvela/ai-sdk-provider/adapters/local`) marked **"development and testing only — not for production key material. Production users: implement your own adapter backed by a hardware wallet, MPC signer, or wallet-standard adapter."** Production bundles that only import the main entry never bundle local-keypair code via tree-shaking, so no runtime-gating is required — browser/Edge consumers physically cannot reach key material through the main entry.

**Package exports** (see §5.1):

```json
{
  "exports": {
    ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" },
    "./adapters/local": { "import": "./dist/adapters/local.js", "types": "./dist/adapters/local.d.ts" },
    "./package.json": "./package.json"
  }
}
```

**What stays from prior revisions:**
- `signPayment` remains the METHOD name on the adapter (matches the existing callback signature in Round 2).
- Request body size cap (default 1 MB) before invoking the adapter.
- AbortSignal propagation through `args.signal`.
- Observability / 402-body-allowlist / error-sanitization / retry-bomb guard concerns.
- Budget reservation/debit state machine (minus its OpaquePrivateKey reference).

**What is removed:**
- `OpaquePrivateKey` branded type and `privateKeyFromBase58`.
- The runtime-gate module (`src/runtime-gate.ts`) and `assertNodeRuntime()` calls — adapter authors are responsible for their own runtime guarantees.
- Hybrid precedence resolution and the conflict warn-once.
- The `.refine(atLeastOne)` zod check.

**Impact if changed later:** moderate. Adapter interface evolution is a breaking change; additive interface methods are not.

### 3.5 Initial model coverage — all 26+ vs curated subset

| Option | Coverage | Pros | Cons |
|---|---|---|---|
| A | All 26+ from `config/models.toml` via build-time codegen into `SolvelaModelId = 'claude-sonnet-4-5' \| 'gpt-4o' \| ... \| (string & {})` (enum + Vercel-idiom escape-hatch tail). | Full coverage; TS autocomplete on known models; `(string & {})` tail accepts any valid string at runtime without TypeScript error, so a gateway shipping a new model never breaks consumers stuck on an older package version. Matches the first-party Vercel idiom. | Requires codegen script; union size ~26 — fine. |
| B | Hand-maintained curated subset of 6-8 "headline" models; rest accessed as plain strings. | Smaller surface. | Drift risk; no autocomplete on 75%. |
| C | `SolvelaModelId = string`. | Zero maintenance (OpenRouter uses this because 300+ models makes union impractical). | No autocomplete; typos ship. |

**Recommendation: Option A (codegen + `(string & {})` tail) — Round 3 refinement.** TS autocomplete is a primary selling point; `config/models.toml` is the gateway's source of truth per CLAUDE.md. The `(string & {})` tail is the first-party Vercel idiom — enumerated members give IDE autocomplete for known models while preserving runtime-string flexibility. Solvela's 26-model catalog is enumerable with this safety tail (OpenRouter's bare `string` is impractical at 300+ models, but unnecessarily lossy at our scale).

**Codegen path (T3 polish):** the script resolves `config/models.toml` via `process.env.SOLVELA_MODELS_TOML` with default `path.resolve(__dirname, '../../../config/models.toml')` (three `..` — script lives at `sdks/ai-sdk-provider/scripts/generate-models.ts`; repo root is three levels up). Matches Phase 5 WI-1. Environments where the repo layout differs (e.g., vendored copy) can override via env var without editing the script.

**Impact if changed later:** low. Widening a literal union is non-breaking.

### 3.6 Streaming retry semantics — include in v1 or generate-only

| Option | v1 scope | Pros | Cons |
|---|---|---|---|
| A | Streaming retry in v1. | Research confirms 402 arrives before SSE data flows; clean interception [research §4 "Streaming with Retry After 402"]. | One extra IT scenario. |
| B | Generate-only in v1; streaming in v1.1. | Smaller initial scope. | Strictly worse UX; every modern agent framework streams. |

**Recommendation: Option A (include).**

**Impact if changed later:** none — effort, not architecture.

### 3.7 Documentation destination — README only vs README + docs site

| Option | Destination | Pros | Cons |
|---|---|---|---|
| A | Package README only. | Simplest; npm renders it. | Users searching docs site find nothing. |
| B | Both: README is single source of truth; docs site embeds it. | Discoverability. | README-as-source-of-truth invites drift maintenance. |
| C | **Both, ownership inverted (recommended):** README = minimal quickstart + install + link to full docs. `dashboard/content/docs/sdks/ai-sdk.mdx` = canonical reference, authored independently, NOT synced from README. | Matches Stripe, Supabase, and every Vercel community provider. README serves the npm-page entry point; docs site serves the canonical reference. No drift maintenance. Matches developer expectations for payments-adjacent SDKs. | Two documents to author at v0.1; ownership boundary must be communicated to writers. |

**Recommendation: Option C (Round 3 reversal).** Stripe, Supabase, and every Vercel community provider follow this pattern. The current Fumadocs site lives at `/home/kennethdixon/projects/Solvela/dashboard/content/docs/` (verified — see `dashboard/content/docs/sdks/{typescript,python,go,rust,mcp}.mdx`). Phase 9 produces both: (a) a minimal `README.md` (install + 10-line quickstart + link to docs site) and (b) `dashboard/content/docs/sdks/ai-sdk.mdx` as canonical reference (features, options reference, adapter-authoring guide, migration-from-openai-sdk, error reference, troubleshooting). Different audiences, different depths. The adjacent `docs.solvela.ai` migration (separate workstream, separate window) will pick up the MDX when it runs; this plan does **not** block on that migration. The broken `/rcr-docs-site/` path from the prior plan draft is removed.

**Impact if changed later:** low.

### 3.8 NEW — ESM/CJS interop strategy for `@solvela/sdk` consumption (T1-D)

**Context:** The new provider is ESM-only (`"type": "module"`). The existing `@solvela/sdk` is CJS (no `"type": "module"`, no `exports` field) and uses internal `require('@solana/web3.js')` / `require('@solana/spl-token')` / `require('bs58')` calls (see `sdks/typescript/src/x402.ts` L168-L172, L321-L326). Under Next.js / Turbopack bundling with ESM consumers, `await import('@solvela/sdk')` may not resolve cleanly, and the internal `require()` calls may be statically rejected or fail at runtime on Vercel Edge / bundled environments.

**Spike (Phase 1, MANDATORY before Phase 4):** verify the following three scenarios. The pass-bar requires success in BOTH the bare Node 20 ESM scenario AND the Next.js 16 `next build` Turbopack scenario. The Webpack fallback is informational only.

1. **Bare Node 20 ESM** (blocker): `node --import tsx/esm test.ts` — `await import('@solvela/sdk')` resolves; `sdk.createPaymentHeader` exists and is callable; the internal `require('@solana/web3.js')` resolves when the ESM consumer has the peer installed.
2. **Next.js 16 `next build` via Turbopack** (blocker per Round 3): a minimal app-router app importing `@solvela/sdk` through the new provider builds clean. Note: this scenario is highly likely to fail per open Next.js issue #78267 (Turbopack failing on CJS packages with runtime `require()` inside function bodies). If it fails, the spike counts as **failed** and §3.8 Option B (extract `@solvela/signer-core`) is triggered.
3. **Next.js 16 `next build` via Webpack fallback** (informational): same minimal app, `next build --no-turbo`. Records whether Webpack succeeds where Turbopack fails — useful diagnostic, but not a pass condition since Turbopack is the Next.js 16 default.

**If both blocker scenarios pass:** no change needed. Proceed with `@solvela/sdk` as a peerDependency.

**If either blocker fails:** pick one of:

| Option | Approach | Pros | Cons |
|---|---|---|---|
| A | Republish `@solvela/sdk` as dual ESM+CJS (add `"exports"` field, Rollup/tsup dual build). | Fixes the root cause; benefits all ESM consumers of `@solvela/sdk`. | Touches `sdks/typescript/` — pulls scope into this plan. |
| B | Extract signing into a new ESM-native `@solvela/signer-core` consumed by both this provider and `@solvela/sdk`. | Keeps existing `@solvela/sdk` untouched; clean separation. | New package; extra publish + maintenance. |
| C | Inline the signing logic into this provider (duplicate `createPaymentHeader`). | Zero cross-package dependency. | Code duplication; drift risk when `@solvela/sdk` updates. |

**Recommendation: Option B (`@solvela/signer-core`)** if the spike fails; Option A only if the user wants to upgrade `@solvela/sdk` anyway. Option C is the fallback if B is rejected.

**Approval Required:** planner recommends B; user must approve before Phase 4 if the Phase 1 spike fails.

**Impact if changed later:** moderate — a new package creates migration churn.

### 3.9 NEW — `@solana/web3.js` 1.x vs `@solana/kit` (T2-L)

**Context:** `@solvela/sdk` currently pins `@solana/web3.js ^1.87.0` (see `sdks/typescript/package.json` L17-L24). The repo's `solana-dev` skill prefers `@solana/kit` for new client code. If `@solvela/sdk` migrates to `@solana/kit` during this plan's maintenance lifetime, the default-signer adapter breaks.

| Option | Path | Pros | Cons |
|---|---|---|---|
| A | Route default signer through `@solvela/sdk` (inherits its web3.js 1.x lock). | Reuse; no duplicate signing code. | Coupled to `@solvela/sdk`'s migration cadence. |
| B | Route default signer directly through `@solana/kit` in this package. | Future-proof; aligns with `solana-dev` skill. | Duplicates signing logic from `@solvela/sdk`; two codepaths to maintain. |

**Recommendation: Option A** for v1.0 *unless* §3.8 Option B triggers. Set a review trigger: "revisit when `@solvela/sdk` declares `@solana/kit` as a peer dep or migrates its internal imports to `@solana/kit`."

**Round 3 conditional (`@solana/kit`-first for new signer-core):** **if §3.8 Option B triggers creating `@solvela/signer-core`, that package MUST be built on `@solana/kit` directly, not web3.js 1.x.** Rationale: `@solana/kit` is production-stable at v6.8.0 (April 10, 2026). `@solana-program/token` provides kit-native `TransferChecked` instruction builder without pulling web3.js transitively. The repo's own `solana-dev` skill mandates "new code: Kit-first." Building `@solvela/signer-core` on web3.js 1.x would create a maintenance liability the moment `@solvela/sdk` itself migrates. Reuse-existing-`@solvela/sdk` (web3.js 1.x) is acceptable for v1.0; greenfield `@solvela/signer-core` is not.

**Impact if changed later:** low — swap the internal import path; public callback API unchanged.

### 3.10 NEW — Legacy `RCR_` env prefix compatibility (T3)

| Option | Path | Pros | Cons |
|---|---|---|---|
| A | Accept `RCR_API_URL` with a deprecation warning (mirrors `@solvela/sdk`). | Easier migration for users porting from `@solvela/sdk`. | Imports legacy baggage into a brand-new Solvela-native package. |
| B | Solvela-native only (`SOLVELA_*`); no `RCR_*` fallback. | Clean slate; no legacy baggage in a 2026 package. | Users who still have `RCR_API_URL` set must rename. |

**Recommendation: Option B.** The legacy prefix belongs in existing packages during the rebrand window. A package shipping fresh in 2026 should not inherit it.

**Impact if changed later:** low — add a warn-on-read fallback if users complain.

---

## 4. Architecture overview

### 4.1 Public API surface

The published package exports exactly one factory and a small set of types. Real TypeScript signatures — field names and `createOpenAICompatible` interop come from `packages/openai-compatible/src/openai-compatible-provider.ts` [research §2]:

```typescript
// sdks/ai-sdk-provider/src/index.ts (conceptual)
import type { LanguageModelV3 } from '@ai-sdk/provider';
import { UnsupportedFunctionalityError } from '@ai-sdk/provider';
import type { FetchFunction } from '@ai-sdk/provider-utils';

/** Public adapter interface — every signer is an adapter. See §3.4. */
export interface SolvelaWalletAdapter {
  /** Adapter identity for logs/metrics. e.g. "local-test-keypair", "phantom", "ledger". */
  readonly label: string;
  /** Sign a parsed 402 payment-required and return the base64 PAYMENT-SIGNATURE header value. */
  signPayment(args: {
    paymentRequired: SolvelaPaymentRequired;
    resourceUrl: string;
    requestBody: string;
    signal?: AbortSignal;
  }): Promise<string>;
}

export interface SolvelaProviderSettings {
  /** Gateway base URL. Defaults to process.env.SOLVELA_API_URL or https://api.solvela.ai/v1.
   *  NOTE: the `/v1` suffix is MANDATORY — see §4.3 baseURL handling. */
  baseURL?: string;

  /** Optional scope token for per-wallet API key tagging. NOT a signing key.
   *  Forwarded as a header to the gateway; never used to derive signatures. */
  apiKey?: string;

  /** REQUIRED. Adapter implementing SolvelaWalletAdapter. No escape hatch — every
   *  signer is an adapter; raw private keys are not accepted at this boundary.
   *  See §3.4 (Round 3 reversal) and adapter authoring guide in docs MDX.
   *  For dev/test, import `createLocalWalletAdapter` from
   *  `@solvela/ai-sdk-provider/adapters/local`. Production users supply their own. */
  wallet: SolvelaWalletAdapter;

  /** Optional static headers merged into every request. PAYMENT-SIGNATURE is filtered out
   *  if present here, and the filter emits a `console.warn` once per provider instance. */
  headers?: Record<string, string>;

  /** Session budget in USDC; throws SolvelaBudgetExceededError when exhausted. Uses a
   *  reservation/debit algorithm (see §4.3 budget) to prevent TOCTOU races. */
  sessionBudget?: bigint;

  /** Cap on `init.body` length before signing. Default 1_000_000 bytes (1 MB). */
  maxBodyBytes?: number;

  /** If true, allows non-HTTPS baseURL. Required for tests against localhost.
   *  REJECTED in production (NODE_ENV === 'production' or VERCEL_ENV === 'production'
   *  or on Vercel Edge runtime). Default: false. */
  allowInsecureBaseURL?: boolean;

  /** Override fetch (for tests / observability). Wraps — does not replace — the 402 logic. */
  fetch?: FetchFunction;

  /** Forwarded to openai-compatible. Default: false (conservative — many upstream models
   *  do not support structured outputs). Callers opt in per model. */
  supportsStructuredOutputs?: boolean;
}

export interface SolvelaProvider {
  (modelId: SolvelaModelId | (string & {})): LanguageModelV3;
  chat(modelId: SolvelaModelId | (string & {})): LanguageModelV3;
  textEmbeddingModel(_id: string): never; // throws UnsupportedFunctionalityError
  imageModel(_id: string): never;         // throws UnsupportedFunctionalityError
}

export function createSolvelaProvider(settings: SolvelaProviderSettings): SolvelaProvider;

/** Default singleton, like @ai-sdk/openai's `openai`. */
export const solvela: SolvelaProvider;

export type SolvelaModelId = /* generated union from config/models.toml + (string & {}) tail */ string;

export type { SolvelaWalletAdapter };

export {
  SolvelaPaymentError,
  SolvelaBudgetExceededError,
  SolvelaSigningError,
  SolvelaInvalidConfigError,
  SolvelaUpstreamError,
} from './errors';
```

Reference adapter (separate entry point — see §3.4):

```typescript
// @solvela/ai-sdk-provider/adapters/local
// DEV/TEST ONLY — not for production key material.
import type { Keypair } from '@solana/web3.js';
import type { SolvelaWalletAdapter } from '@solvela/ai-sdk-provider';

export function createLocalWalletAdapter(keypair: Keypair): SolvelaWalletAdapter;
```

Consumer call site:

```typescript
import { generateText } from 'ai';
import { createSolvelaProvider } from '@solvela/ai-sdk-provider';
import { createLocalWalletAdapter } from '@solvela/ai-sdk-provider/adapters/local'; // DEV/TEST ONLY
import { Keypair } from '@solana/web3.js';
import bs58 from 'bs58';

const keypair = Keypair.fromSecretKey(bs58.decode(process.env.SOLANA_WALLET_KEY!));

const solvela = createSolvelaProvider({
  baseURL: 'https://api.solvela.ai/v1',
  wallet: createLocalWalletAdapter(keypair),
  sessionBudget: 500_000n, // 0.50 USDC in atomic units
});

const { text } = await generateText({
  model: solvela('claude-sonnet-4-5'),
  prompt: 'Explain the x402 protocol.',
});
```

Production users implement their own `SolvelaWalletAdapter` backed by a hardware wallet, MPC signer, or wallet-standard adapter — and import only the main entry point. Tree-shaking ensures local-keypair code never reaches their bundle.

### 4.2 Internal component boundaries

```
sdks/ai-sdk-provider/
├── src/
│   ├── index.ts                        # re-exports; zero logic
│   ├── provider.ts                     # createSolvelaProvider factory
│   ├── config.ts                       # SolvelaProviderSettings validation + defaults
│   ├── wallet-adapter.ts               # SolvelaWalletAdapter interface (public type)
│   ├── fetch-wrapper.ts                # 402 -> sign -> retry loop; error sanitizer
│   ├── adapters/
│   │   └── local.ts                    # createLocalWalletAdapter — dev/test reference (sub-export)
│   ├── errors.ts                       # typed errors extending APICallError / AISDKError
│   ├── generated/
│   │   └── models.ts                   # auto-generated from config/models.toml
│   ├── budget.ts                       # reserve/finalize/release budget state machine
│   └── util/
│       ├── parse-402.ts                # gateway's { error: { type, message: JSON } } envelope
│       ├── redact.ts                   # base58/hex sanitisation for error surfaces
│       └── warn-once.ts                # memoized console.warn
├── scripts/
│   └── generate-models.ts              # pre-build codegen
├── tests/
│   ├── unit/                           # see §7
│   ├── integration/                    # see §7
│   └── fixtures/
│       ├── 402-envelope.json           # snapshot of gateway 402 (mirrors crates/gateway/tests/)
│       └── sentinel-key.base58         # non-funded test keypair for leakage tests
├── .github/workflows/ai-sdk-provider.yml  # Node 18 + 20 CI, size-limit
├── package.json
├── tsconfig.json
├── tsup.config.ts
├── vitest.config.ts
└── README.md
```

### 4.3 Mapping to Vercel AI SDK stack

| Component | AI SDK plug-in point | Cite |
|---|---|---|
| `createSolvelaProvider` factory | Wraps `createOpenAICompatible` with a fixed `baseURL` (normalised to end in `/v1` — see below), generated `name`, precomputed `fetch`, and `transformRequestBody: undefined`. | [research §2] |
| `fetch-wrapper.ts` | Passed as `fetch:` in `OpenAICompatibleProviderSettings`. Replaces `globalThis.fetch` only inside this provider's HTTP path [research §4]. | [research §4.1] |
| `wallet-adapter.ts` + `adapters/local.ts` (sub-export) | Public `SolvelaWalletAdapter` interface; reference dev/test `createLocalWalletAdapter` ships from `./adapters/local` sub-path. Production users implement their own adapter. See §3.4 (Round 3 reversal). | `sdks/typescript/src/x402.ts` L34-L91 |
| `errors.ts` | All errors extend `APICallError` or `AISDKError` for uniform `APICallError.isInstance(err)`. `isRetryable: false` for 402-with-budget-exceeded, `true` for network errors [research §1]. | [research §1] |
| `budget.ts` | Reserve at 402 parse; finalize at retry 200; release on every error path. Prevents TOCTOU overspend (T1-A). | §4.3 budget. |
| `generated/models.ts` | Consumed by TS overload `solvela(modelId: SolvelaModelId): LanguageModelV3`. | CLAUDE.md "Configuration". |
| `provider.ts` `supportedUrls` | Returns `{}` (v1 scope: chat text + tools only — see §1 Out of Scope). | — |

**baseURL handling (T2-B fix).** `createOpenAICompatible` builds request URLs as `${baseURL}/chat/completions`. Solvela's endpoint is `/v1/chat/completions`. The config resolver MUST normalize `baseURL`:
1. If `baseURL` provided by user, it MUST end in `/v1` OR `/v1/` (strip trailing slash). If missing, append `/v1`.
2. Default `baseURL` is `https://api.solvela.ai/v1`.
3. Unit test Unit-1 asserts the URL reaching the underlying fetch is exactly `https://api.solvela.ai/v1/chat/completions` for the default case, and the user-provided `https://custom.example.com` becomes `https://custom.example.com/v1/chat/completions`.

**Fetch-wrapper invariants (T2-D fix):**
- The wrapper never reads `resp.body` before deciding to return it; only `resp.status` is inspected on the 200-path. This prevents accidental tee-buffering of SSE streams.
- No mid-stream retry. A 200 response followed by a stream-error chunk surfaces as an SDK error; the wrapper is not re-entered. See IT-11.
- Caller-supplied `PAYMENT-SIGNATURE` header (T2-F fix): if `init.headers['PAYMENT-SIGNATURE']` or any case-variant is present, the wrapper MUST NOT re-sign on 402. It surfaces the 402 as `SolvelaPaymentError` directly with an explanatory message ("caller supplied PAYMENT-SIGNATURE; refusing to re-sign").

**Scheme selection from `accepts[]` (P1-4 fix).** A 402 body's `accepts[]` array may contain multiple payment options (different assets, different amounts, different schemes). **v1 rule:** the provider selects the **first `accepts[]` entry whose `scheme === 'exact'` AND `asset === USDC`** (USDC-SPL). `cost` used by the budget state machine is that entry's `amount`. If no matching entry exists, the provider throws `SolvelaPaymentError('no supported payment scheme in accepts[]: v1 requires scheme=exact + asset=USDC')`. Future v2 may add a scheme-selection policy option (e.g., `preferScheme: 'escrow'`) to `SolvelaProviderSettings`. Unit-2 asserts that a 402 body with multiple `accepts[]` entries (USDC-SPL exact first, USDC-SPL escrow second, USDC-SPL exact on a different chain third) selects the first matching entry correctly; a second case with zero matches throws the typed error.

**Budget reservation/debit state machine (T1-A fix):**

```
state: { available: number, reserved: Map<requestId, number> }
on 402 parsed, cost = selected_accept.amount  // per scheme-selection rule above
  if (available - sumReserved() < cost) throw SolvelaBudgetExceededError  // atomic check+reserve
  reserved.set(requestId, cost)
on retry success (2xx):
  available -= reserved.get(requestId); reserved.delete(requestId)  // finalize
on retry failure / abort / signing error / network error:
  reserved.delete(requestId)  // release
```

Implementation uses a single synchronous critical section per provider instance (JS is single-threaded; no mutex needed) wrapping the check-and-reserve step. `requestId` is a per-invocation UUID. Unit-6 enumerates two scenarios: (a) two concurrent invocations, budget affords only one — exactly one succeeds, one throws `SolvelaBudgetExceededError`; (b) cancellation mid-retry — reservation released, budget visible to next call.

**Signer invocation + `PAYMENT-SIGNATURE` header name (T1-B fix):**
The gateway middleware reads the header case-insensitively via `headers().get("payment-signature")` at `crates/gateway/src/middleware/x402.rs` L38. The canonical casing emitted by the provider is the uppercase `PAYMENT-SIGNATURE` (standard HTTP practice and matches existing `@solvela/sdk` output). Both work on the wire; we emit the canonical form. This is cited in the fetch-wrapper source comment.

**Gateway 402 envelope (T1-B fix).** The gateway emits **only** the envelope:

```json
{
  "error": {
    "type": "invalid_payment",
    "message": "<JSON-stringified PaymentRequired>"
  }
}
```

Source of truth: `/home/kennethdixon/projects/Solvela/crates/gateway/src/error.rs` lines 75-80. `error.type` is one of `payment_required` (no body detail) or `invalid_payment` (body detail JSON). The direct `{x402_version, accepts, ...}` shape the prior plan draft also supported is NOT emitted by the gateway. `parse-402.ts` targets the envelope only. Phase 1 adds the cross-repo contract fixture.

**Error surface sanitization (T1-C fix — `APICallError` leakage).** `APICallError` (from `@ai-sdk/provider`) carries `responseHeaders`, `responseBody`, `requestBodyValues`, `stack`, `cause`. Without sanitization, a signed `PAYMENT-SIGNATURE` value would land in observability systems (Sentry / Vercel Observability / OTel) whenever `postJsonToApi` converts a non-2xx response into `APICallError`.

**Seam choice — Option A (wrapper inspects non-2xx before return).** The fetch wrapper runs *inside* `postJsonToApi`; it returns a `Response` to `postJsonToApi`, which then converts non-2xx responses into `APICallError`. The wrapper cannot catch that throw (it happens upstream of the wrapper's return). Instead, the wrapper inspects non-2xx retry responses BEFORE returning them and handles sensitive cases directly:

1. On the retry leg, after `baseFetch(url, { ...init, headers: { ...init.headers, 'PAYMENT-SIGNATURE': header }, signal })`, inspect `resp.status`:
   - **2xx**: finalize budget; return `resp` unmodified. `postJsonToApi` surfaces it normally. (The 200 path never reads `resp.body`; see T2-D.)
   - **402**: retry-bomb guard — release reservation; throw `SolvelaPaymentError("Payment rejected after retry")` directly (bypasses `postJsonToApi` error conversion).
   - **Any other non-2xx (e.g., upstream LLM 500)**: release reservation; read the body (non-streaming error envelope — safe to buffer), then **throw a `SolvelaUpstreamError extends APICallError`** constructed directly by the wrapper. The wrapper passes only sanitized fields into the constructor: `statusCode`, a redacted `responseBody`, `responseHeaders` with `PAYMENT-SIGNATURE` stripped (via `stripPaymentSignature`), and `requestBodyValues: undefined`. Because the wrapper throws directly, `postJsonToApi` never sees the raw retry response and never constructs a vanilla `APICallError` carrying the signature.
2. Every Solvela error constructor (`SolvelaPaymentError`, `SolvelaSigningError`, `SolvelaBudgetExceededError`, `SolvelaInvalidConfigError`, `SolvelaUpstreamError`) runs `sanitizeError` on its inputs (T1-C defense in depth).
3. The only remaining `APICallError` surface is from `postJsonToApi` itself on the **first** (pre-retry) fetch — which never carries `PAYMENT-SIGNATURE` because the first request does not include it.
4. The README Sentry `beforeSend` snippet (§6 Phase 9) is documented as defense-in-depth for the case where a caller catches a Solvela error and re-throws it wrapped in something else.

**Request body size cap (T2-C fix).** `init.body` can theoretically be `ReadableStream`, `Blob`, `FormData`, `Uint8Array`, or string. V1 scope is chat + tools; `createOpenAICompatible` always passes a JSON string. The wrapper enforces:
1. Before calling signer, `typeof init.body === 'string'` — otherwise throw `SolvelaPaymentError('unsupported body type for payment signing in v1')`.
2. `init.body.length <= 1_000_000` bytes (1 MB default) — otherwise throw `SolvelaPaymentError('request body exceeds payment signing size cap')`.
3. Cap is configurable via a private `SOLVELA_MAX_SIGNED_BODY_BYTES` option (not exposed in public settings yet).

**AbortSignal propagation (T2-E fix).**
- `init.signal` is forwarded to the signer callback as `args.signal`.
- `init.signal` is forwarded to the retry `fetch(url, { ...init, signal })`.
- If abort fires between sign and retry (signed tx built, not submitted), the wrapper emits a one-time `console.warn('[solvela] aborted mid-retry — signed transaction built but not submitted; wallet may be in an uncertain state until blockhash expires (~60-90s)')`. The warning contains NO signature bytes.
- Phase 11 smoke test includes an abort-mid-retry scenario and verifies no double-spend on-chain.

**Caller-supplied `PAYMENT-SIGNATURE` (T2-F).** See invariants above.

**402 body allowlist (T2-G).** `SolvelaPaymentError.responseBody` is populated from an explicit allowlist:

```typescript
const ALLOWED_402_FIELDS = {
  x402_version: 'number',
  accepts: [{ scheme: 'string', pay_to: 'string', amount: 'string', asset: 'string',
              escrow_program_id: 'string?', max_timeout_seconds: 'number?' }],
  cost_breakdown: { total: 'string', breakdown: 'object?' },
} as const;
```

Every other field (e.g., a future `internal_trace_id`) is dropped. Unit-2 asserts a 402 body with extra `internal_trace_id` yields `err.responseBody` without that key.

**`SOLVELA_ALLOW_INSECURE_BASE_URL` production guard (T2-H).**
- If `process.env.SOLVELA_ALLOW_INSECURE_BASE_URL === 'true'` AND (`process.env.NODE_ENV === 'production'` OR `process.env.VERCEL_ENV === 'production'`): emit `console.error` at provider construction AND refuse to apply the override (TLS remains enforced).
- On Vercel Edge runtime (detected via `typeof EdgeRuntime !== 'undefined'` or equivalent), the env var is ignored unconditionally. TLS is always enforced in Edge.
- Unit test Sec-11 covers all three paths (prod + var set, edge + var set, dev + var set).

**Test-mode env (T3 — M5).** `SOLVELA_AI_SDK_PROVIDER_TEST_MODE` is honoured ONLY when `process.env.NODE_ENV === 'test'` AND the `baseURL` hostname is `localhost` or `127.0.0.1`. Any other combination: the env var is ignored and TLS is enforced.

---

## 5. Dependencies and technology inventory

Per `~/.claude/rules/common/development-workflow.md`.

### 5.1 npm packages

| Package | Version / range | Dependency type | Purpose | Source |
|---|---|---|---|---|
| `@ai-sdk/provider` | `^3.0.0` (V3 — stable channel; Round 3 reversal) | peerDependency | Core interface types | [research §3] |
| `ai` | `^6.0.0` (V3 — stable channel; Round 3 reversal) | peerDependency | Top-level SDK fns | [research §1] |
| `@ai-sdk/openai-compatible` | **exact-pin latest stable 2.x** (V3 channel; document the exact pinned patch in `package.json`, e.g. `2.0.x`) | dependency | Factory we wrap | [research §2] |
| `@ai-sdk/provider-utils` | `^4.0.0` (V3 — stable channel) | dependency | `FetchFunction`, `postJsonToApi` | [research §1] |
| `zod` | `^3.25.76 \|\| ^4.1.8` | peerDependency | Matches openai-compatible peer range | [research §3] |
| `@solvela/sdk` | `^0.1.0` (or `@solvela/signer-core` — see §3.8) | peerDependency (optional) | Default signer | `sdks/typescript/package.json` |
| `@solana/web3.js` | `^1.87.0` | peerDependency (optional) | Needed only if `wallet:` used | `sdks/typescript/package.json` |
| `@solana/spl-token` | `^0.4.0` | peerDependency (optional) | Same | `sdks/typescript/src/x402.ts` L170 |
| `bs58` | `^5.0.0` | peerDependency (optional) | base58 decode | `sdks/typescript/src/x402.ts` L172 |
| `tsup` | `^8.0.0` | devDependency | ESM build | ecosystem convention [research §3] |
| `vitest` | `^2.0.0` | devDependency | Test runner | |
| `@vitest/coverage-v8` | `^2.0.0` | devDependency | Coverage | |
| `undici` | `^6.0.0` | devDependency | `MockAgent` | |
| `@types/node` | `^22.0.0` | devDependency | matches `sdks/mcp/package.json` | |
| `typescript` | `^5.6.0` | devDependency | | |
| `@iarna/toml` | `^2.2.5` | devDependency | Parse `config/models.toml` | |
| `size-limit` | `^11.0.0` | devDependency | Bundle-size guard (T3) | |
| `@size-limit/preset-small-lib` | `^11.0.0` | devDependency | Same | |

**Pinning strategy (Round 3 update):**
- `@ai-sdk/openai-compatible`: **exact-pin, no caret** to the latest stable 2.x; pinned patch documented in the `package.json` comment + README. Bumped only via an intentional plan. Round 3 reversal: now stable channel, not beta, since §3.1 targets V3.
- Other `peerDependencies` use stable ranges (`^6.0.0`, `^3.0.0`, `^4.0.0`, etc.) — no `>=X.Y.Z-beta` ranges.
- Other `dependencies` use caret (`^`).
- `devDependencies` use caret; resolved via `package-lock.json`.
- **No `postinstall` / `preinstall` / `prepublish` scripts in this package or any direct devDependency added to it.** CI job `guard-install-scripts` runs `npm ls --all --json` and greps the output for install-script presence in new additions; build fails on unexpected scripts.
- `npm audit --production` runs in CI (Phase 10 pre-publish). HIGH/CRITICAL blocks publish.

**`package.json` `exports` and `files` (Round 3 — adapter sub-path).** Production bundles importing only the main entry never bundle local-keypair code via tree-shaking, so no runtime-gating is required. Explicit `exports` map:

```json
{
  "exports": {
    ".": { "import": "./dist/index.js", "types": "./dist/index.d.ts" },
    "./adapters/local": { "import": "./dist/adapters/local.js", "types": "./dist/adapters/local.d.ts" },
    "./package.json": "./package.json"
  },
  "files": ["dist/", "examples/", "README.md", "LICENSE"]
}
```

`examples/` is intentionally shipped so users see inline code without leaving npm. Test fixtures, scripts, tests, and `.env*` remain excluded. Phase 10 WI-1 and Phase 9 WI-2 are aligned with this decision.

### 5.2 Environment variables

| Name | Who sets it | Required? | Purpose |
|---|---|---|---|
| `SOLVELA_API_URL` | consumer | optional (default `https://api.solvela.ai/v1`) | Overrides `baseURL`. Normalized to include `/v1`. |
| `SOLANA_WALLET_KEY` | consumer | adapter-implementation concern only | base58 keypair. **Round 3:** no longer a first-class provider env var; the provider does not read it directly. The reference `createLocalWalletAdapter` example reads it; production adapter implementations read whatever env vars they choose. |
| `SOLANA_RPC_URL` | consumer | adapter-implementation concern only | Solana RPC for blockhash. Read by adapter implementations that need it (e.g., the reference `createLocalWalletAdapter`); not read by the provider itself. |
| `SOLVELA_SESSION_BUDGET` | consumer | optional | Runtime override of `sessionBudget:`. |
| `SOLVELA_TIMEOUT_MS` | consumer | optional | Fetch timeout. |
| `SOLVELA_ALLOW_INSECURE_BASE_URL` | consumer | optional | Same as `allowInsecureBaseURL: true`. **Refuses to apply in production or on Vercel Edge** (T2-H). |
| `SOLVELA_AI_SDK_PROVIDER_TEST_MODE` | test only | test runtime only | Honoured only when `NODE_ENV === 'test'` AND `baseURL` hostname is localhost/127.0.0.1 (T3/M5). |
| `SOLVELA_MODELS_TOML` | codegen only | optional | Override path to `config/models.toml` for the codegen script (T3). |
| `SOLVELA_MAX_SIGNED_BODY_BYTES` | consumer | optional (default 1_000_000) | Cap on `init.body` length before signing (T2-C). |
| `NPM_TOKEN` | publisher (CI) | Phase 10 only | `npm publish` authentication — CI-only, never laptop. |

**Legacy prefix (T3):** per §3.10, the provider does NOT accept `RCR_*` fallback. Solvela-native only.

### 5.3 Credentials & tokens

| Credential | Scope | Used by | Storage |
|---|---|---|---|
| npm automation token (**2FA required**) | `publish` on `@solvela/*` scope | Phase 10 (CI only) | GitHub Actions secret `NPM_TOKEN`. Never committed. Never on laptop. |
| GitHub OIDC (`id-token: write`) | npm `--provenance` signing | Phase 10 | GitHub Actions native. No stored secret. |
| GPG signing key | `git tag -s` | Phase 10 | User-side. |
| GitHub PAT | `repo` (fork `vercel/ai`) | Phase 10 community PR | User's GitHub secret vault. |
| Solana devnet keypair | funded with devnet USDC | Phase 11 (optional) | User-side. Never persisted by executor. |
| Solana mainnet keypair | — | NOT used anywhere. Out of scope. | — |

### 5.4 Skills, hooks, plugins, MCPs

| Skill / tool | Phase(s) | Purpose |
|---|---|---|
| `vercel-plugin:ai-sdk` | 2, 3, 5, 6 | Authoritative Vercel AI SDK patterns. MUST be loaded before touching any `LanguageModelV3` code. |
| `solana-dev` | 4 | Signer adapter (base58 / VersionedTransaction indirectly via `@solvela/sdk`). |
| `security-review` | 3, 4, 8, 10 | Private key handling, TLS, header redaction, runtime gating. |
| `tdd-workflow` | 7 (and throughout) | RED-GREEN-REFACTOR. |
| `api-design` | 2, 6 | 402 response shape + OpenAI compat. |
| `domain-fintech` | 3, 6 | USDC precision; budget arithmetic with no float drift. |
| `pr-review-toolkit:silent-failure-hunter` | 3, 4, 7, 8 | Error-swallow detection. |
| `superpowers:test-driven-development` | 7 | Write-tests-first. |
| `superpowers:dispatching-parallel-agents` | 7, 8 | Parallel test authoring. |
| `superpowers:verification-before-completion` | every "Done criteria" gate | Evidence before assertion. |
| `oh-my-claudecode:verifier` | final, Phase 10 | Completion evidence. |
| `oh-my-claudecode:code-reviewer` | end of Phases 3, 4, 6, 7, 8, 9 | Final review. |
| `rustyclaw-orchestration` | pre-execution | Domain-skill routing. |
| `vercel-plugin:turbopack` | Phase 1 spike (§3.8) | Verify Next.js 16 builds against `@solvela/sdk` CJS. |

**`oh-my-claudecode:test-engineer` verification (T3):** before first use, confirm this agent exists in the OMC catalog. If it does not, substitute `oh-my-claudecode:executor` with an **explicit test-only prompt** (agent MUST still be a different invocation than the implementation executor per saved memory `feedback_test_author_separation.md`).

**`oh-my-claudecode:security-reviewer` verification (P1-5):** before first use in Phases 3, 4, 6, or 10, confirm `oh-my-claudecode:security-reviewer` exists in the OMC catalog. If it does not, substitute `oh-my-claudecode:code-reviewer` with an **explicit security-only prompt covering the Sec-1 through Sec-23 checklist items applicable to the phase under review** (e.g., Phase 3 covers Sec-2 through Sec-4, Sec-11 through Sec-17, Sec-21; Phase 6 covers Sec-2, Sec-3, Sec-15; Phase 10 covers Sec-9, Sec-19, Sec-20, Sec-23). The substitute invocation MUST still be a different invocation than the implementation executor.

**Hooks:** existing repo hooks apply. No new hooks needed.

**MCPs:** none required.

### 5.5 Rules applied

| Rule file | Relevance |
|---|---|
| `~/.claude/rules/common/coding-style.md` | Immutability, small files, error handling. |
| `~/.claude/rules/common/security.md` | Secret management, pre-commit checks. |
| `~/.claude/rules/common/testing.md` | 80% coverage minimum, TDD, unit + integration + e2e. |
| `~/.claude/rules/common/git-workflow.md` | Conventional commits; type `feat`. |
| `~/.claude/rules/common/patterns.md` | Response envelope pattern. |
| `~/.claude/rules/common/development-workflow.md` | This plan's structure mandate. |
| `~/.claude/rules/common/performance.md` | Sonnet default; Opus for Phase 3/4 architectural work. |
| `~/.claude/rules/common/hooks.md` | Auto-accept guidance — see below. |
| `CLAUDE.md` (repo) | Architectural rules 4, 5, 7, 8; conventions. |
| Saved memory `feedback_test_author_separation.md` | Separate test agents per phase. |
| Saved memory `feedback_delegation.md` | Main agent delegates everything. |
| Saved memory `feedback_plugin_build_process.md` | Plugin builds — research-first, specialists only, no main-agent coding. |

**Auto-accept posture (T3/critic P1-14 fix):** auto-accept is **OFF** for every phase that writes code or docs examples — specifically Phases 2, 3, 4, 6, 7, 8, 9 (README and MDX code blocks can contain secrets). Auto-accept may be ON only for Phase 1 scaffolding and Phase 5 codegen script skeleton. Phase 10 publish steps run in CI with explicit approval gates — not auto-accept.

### 5.6 Manual user actions

Consolidated in §2. No new items.

### 5.7 Infrastructure prerequisites

| Requirement | Why |
|---|---|
| Node.js >= 18.17 | `@ai-sdk/openai-compatible` engines. |
| npm >= 9 (or pnpm >= 8 / yarn >= 4) | ESM dual-resolution. |
| Git with GPG signing configured | `git tag -s` for Phase 10. |
| Clean `cargo check` state in repo | Executor should not inherit broken workspace state. |
| `config/models.toml` readable from `sdks/ai-sdk-provider/` | Codegen dependency; override via `SOLVELA_MODELS_TOML`. |
| **No root `package.json` in the repo** — verified (Phase 1 ambiguity resolved, T2-K). The new package is an independent npm project; no workspace integration. |
| GitHub Actions `id-token: write` permission | Phase 10 `npm publish --provenance`. |

---

## 6. Phased implementation plan

Every phase lists: Goal, Agent assignment, Work items, Deliverables, Done criteria, Dependencies. **Phase ordering was revised (T2-A): Phase 6 (error declarations) now runs before Phase 3 (fetch wrapper) because Phase 3 references these error types.**

### Phase 1 — Package scaffolding + contract fixture + ESM/CJS spike

**Goal:** Create `sdks/ai-sdk-provider/` with a valid ESM TypeScript package skeleton; snapshot the gateway 402 envelope into a cross-repo fixture; verify ESM consumption of CJS `@solvela/sdk`.

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=sonnet) — scaffolding.
- Spike executor (work item 8): `oh-my-claudecode:executor` (model=sonnet) — separate invocation; no coding, just verification.
- Reviewer: `oh-my-claudecode:code-reviewer` — validates `package.json` + `tsconfig.json`.

**Skills to load:** `vercel-plugin:ai-sdk`, `tdd-workflow`, `vercel-plugin:turbopack` (for work item 8).

**Work items (each = one agent invocation):**

1. Create directory tree `sdks/ai-sdk-provider/{src,tests/unit,tests/integration,tests/fixtures,scripts,.github/workflows}` and empty placeholder files.
2. Write `package.json` — `"type": "module"`, ESM `exports` with separate `./node` entry guarding wallet code path (T1-F fix), peer/dev deps per §5.1, scripts `build`, `test`, `test:watch`, `typecheck`, `generate-models`, `lint`, `size`. **No `postinstall`/`preinstall`/`prepublish` scripts.**
3. Write `tsconfig.json` — `module: "NodeNext"`, `moduleResolution: "NodeNext"`, `target: "ES2022"`, `strict: true`, `declaration: true`, `declarationMap: true`, `sourceMap: true`.
4. Write `tsup.config.ts` — ESM only, entry `src/index.ts`, dts=true, splitting=false.
5. Write `vitest.config.ts` — node env, coverage v8, exclude `scripts/`, `dist/`, `src/generated/`.
6. Write `.gitignore`, `README.md` (stub), `LICENSE` (MIT), `size-limit` config (50 KB ESM gzip cap — tightenable).
7. Write `.github/workflows/ai-sdk-provider.yml` — CI matrix Node 18 + 20; jobs: `install`, `typecheck`, `lint`, `test-unit`, `test-integration`, `size`, `guard-install-scripts`, `audit` (runs `npm audit --production` — HIGH/CRITICAL fails the job).
8. **Contract fixture (T1-B fix).** Snapshot the current gateway 402 envelope shape into `tests/fixtures/402-envelope.json`: a copy of the response body structure emitted by `crates/gateway/src/error.rs:75-80` for the `InvalidPayment` variant with a canonical JSON-stringified `PaymentRequired` in `error.message`. Also copy the same file into `crates/gateway/tests/fixtures/402-envelope.json`. A companion Rust test (added in a follow-up to this plan, not blocking Phase 1 — flagged as follow-up task in §10) will assert the gateway's actual `.into_response()` output matches the fixture. Either side changing the contract fails CI on both sides.
9. **ESM/CJS spike (T1-D fix, §3.8 — Round 3 expanded pass-bar).** In a disposable scratch dir, run THREE scenarios. The pass-bar requires success in BOTH (a) and (b); (c) is informational only.
   - **(a) Bare Node 20 ESM (BLOCKER):** Create a bare Node 20 ESM project. Install `@solvela/sdk@0.1.0` from the local workspace. Run `node --import tsx/esm test.ts` that does `await import('@solvela/sdk')` and asserts `sdk.createPaymentHeader` is callable (pass dummy 402 body; expect a throw or a stub). Record PASS/FAIL.
   - **(b) Next.js 16 `next build` via Turbopack (BLOCKER, Round 3):** Create a disposable Next.js 16 app-router project (`npx create-next-app@latest`). Install `@solvela/sdk` as a direct dependency. In a server route, `const { createPaymentHeader } = await import('@solvela/sdk')`. Run `next build` (Turbopack default per `vercel-plugin:turbopack`). Record whether the build succeeds AND whether the route runtime can resolve `require('@solana/web3.js')`. **Note:** highly likely to fail per open Next.js issue #78267 (Turbopack failing on CJS packages with runtime `require()` inside function bodies).
   - **(c) Next.js 16 `next build` via Webpack fallback (INFORMATIONAL):** Same minimal app, `next build --no-turbo`. Record whether Webpack succeeds where Turbopack fails — useful diagnostic, but NOT a pass condition since Turbopack is the Next.js 16 default.
   - Write findings to `.omc/plans/open-questions.md` under "ai-sdk-provider Phase 1 spike results" with PASS/FAIL per sub-scenario. **If either blocker (a or b) FAILs:** surface `<remember>` and pause for user to choose §3.8 option (planner recommends Option B: `@solvela/signer-core` built on `@solana/kit` per §3.9 Round 3 conditional).

**Deliverables:** directory structure + config files + CI workflow + fixture + spike report.

**Done criteria:**
- `cd sdks/ai-sdk-provider && npm install` succeeds on clean Node 20.
- `npm run typecheck` passes.
- `npm run build` produces `dist/index.js` and `dist/index.d.ts`.
- Reviewer sign-off on package.json peer ranges + exports field wallet-gate.
- Fixture file present in both `sdks/ai-sdk-provider/tests/fixtures/` and `crates/gateway/tests/fixtures/`.
- Spike report written; user notified if §3.8 decision is required.

**Dependencies:** §3.1, §3.2, §3.3 approved.

---

### Phase 2 — Core provider factory + typed options

**Goal:** Implement `createSolvelaProvider(settings)` wrapping `createOpenAICompatible`, with runtime-validated settings via zod and adapter-based wallet configuration (via `SolvelaWalletAdapter` contract).

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=sonnet).
- Tests (separate agent per saved memory): `oh-my-claudecode:test-engineer` (or fallback — see §5.4).
- Reviewer: `oh-my-claudecode:code-reviewer`.

**Skills to load:** `vercel-plugin:ai-sdk`, `api-design`, `tdd-workflow`, `security-review`.

**Work items (Round 3 simplified — adapter interface replaces opaque-key + runtime-gate):**

1. `src/wallet-adapter.ts` — Public `SolvelaWalletAdapter` interface (just the type, no implementation). Exports the type only.
2. `src/config.ts` — zod schema for `SolvelaProviderSettings`. `wallet` validated via `z.custom<SolvelaWalletAdapter>((v) => typeof v === 'object' && v !== null && typeof (v as any).signPayment === 'function' && typeof (v as any).label === 'string')`. **No `.refine(atLeastOne)` needed** — the schema requires `wallet` directly. Default URL resolution (explicit setting > `SOLVELA_API_URL` > `https://api.solvela.ai/v1`). Normalizes `baseURL` to end with `/v1` (T2-B). Rejects non-HTTPS `baseURL` unless `allowInsecureBaseURL` or test-mode (Sec-1, Sec-11, M5). Throws `SolvelaInvalidConfigError` on validation failure.
3. `src/provider.ts` — `createSolvelaProvider(settings)` that:
   - Validates config (zod throws `SolvelaInvalidConfigError` if `wallet` is missing or shape-invalid).
   - Calls `createOpenAICompatible({ name: 'solvela', baseURL: normalizedBaseUrl, fetch: <placeholder>, headers: filtered, supportsStructuredOutputs: settings.supportsStructuredOutputs ?? false })`.
   - `headers` filter: strips any caller-provided `PAYMENT-SIGNATURE` key with a one-time warn.
   - Wiring of real `fetch` comes in Phase 3.
   - **No runtime-gate, no precedence resolution, no warn-once on conflict** — the adapter interface eliminates all three.
4. `src/util/warn-once.ts` — memoized `console.warn` keyed by message string (still used for `headers` filter and some adapter debug emissions).
5. `src/index.ts` — re-export `createSolvelaProvider`, default `solvela` singleton, `SolvelaWalletAdapter` type re-export, error type re-exports.
6. Unit tests (separate agent): config defaults and normalization; missing-wallet rejection (zod); shape-invalid wallet rejection (object missing `signPayment` or `label`); non-HTTPS rejection matrix (dev, prod, edge, test-mode); factory shape (`specificationVersion: 'v3'`); `headers` filter strips `PAYMENT-SIGNATURE` with warn-once.

**Deliverables:** `src/config.ts`, `src/provider.ts`, `src/wallet-adapter.ts`, `src/util/warn-once.ts`, `src/index.ts` (partial), unit tests. **Removed from prior revision:** `src/runtime-gate.ts`, `src/wallet-key.ts`.

**Done criteria:** unit tests green; `createSolvelaProvider({} as any)` (missing wallet) throws `SolvelaInvalidConfigError` at **construction time** (raised by zod). Test assertion: `expect(() => createSolvelaProvider({} as any)).toThrow(SolvelaInvalidConfigError)`. The Phase 2 provider factory never constructs a live `fetch` wrapper in this case — the error fires at settings validation. `generateText` is not invoked in the done-criterion assertion. Reviewer sign-off on the zod schema (especially `wallet` `z.custom` predicate).

**Dependencies:** Phase 1 (incl. spike result if §3.8 applies); Phase 6 (error class declarations) must complete first — see revised order.

---

### Phase 6 — Error mapping (MOVED BEFORE Phase 3)

**Revision note:** Moved before Phase 3 per T2-A. Phase 3 references `SolvelaPaymentError`, `SolvelaBudgetExceededError`, `SolvelaSigningError`; they must exist first.

**Goal:** Declare every typed error used by the wrapper, budget, and signer modules. Ensure `APICallError.isInstance` recognizes each.

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=sonnet).
- Reviewer: `oh-my-claudecode:code-reviewer` + `oh-my-claudecode:security-reviewer` (sanitization review for `SolvelaSigningError`).
- Tests (separate agent): `oh-my-claudecode:test-engineer`.

**Skills to load:** `vercel-plugin:ai-sdk`, `api-design`, `security-review`.

**Work items:**

1. `src/util/redact.ts` — pure functions:
   - `redactBase58(s: string): string` — scrubs any base58-shaped substring (44-88 chars of `[1-9A-HJ-NP-Za-km-z]`) by replacing with `[REDACTED]`.
   - `redactHex(s: string): string` — scrubs any 64+ char hex run.
   - `sanitizeError(err: unknown): SanitizedError` — walks `message`, `stack`, `cause`, `responseHeaders`, `responseBody`, `requestBodyValues`; returns a copy with all redacted. Used by every Solvela error constructor AND the fetch-wrapper's `APICallError` pass-through.
   - `stripPaymentSignature(headers: Record<string,string>): Record<string,string>` — case-insensitive delete of `payment-signature` key. Used by the wrapper error-rewrapping path.
2. `src/errors.ts`:
   - `SolvelaPaymentError extends APICallError` (402 failures; `isRetryable: false`). Constructor runs `sanitizeError` on inputs.
   - `SolvelaBudgetExceededError extends APICallError` (`isRetryable: false`, `statusCode: 402`). Constructor sanitizes.
   - `SolvelaSigningError extends APICallError` (wraps signer failure; `isRetryable: false`). Constructor MUST call `redactBase58` + `redactHex` on the wrapped upstream `err.message` before surfacing (M3).
   - `SolvelaInvalidConfigError extends AISDKError` (construction-time; no `statusCode`). **Constructor also runs `sanitizeError` on its `message` and any context payload (P1-3 consistency fix)** — malformed `baseURL` strings or misrouted settings objects could carry base58-shaped values (e.g., a caller accidentally passing a private key as a URL); sanitization prevents those from landing in error surfaces.
   - `SolvelaUpstreamError extends APICallError` (T1-C Option A seam; wraps non-2xx retry responses other than 402; `isRetryable` derived from `statusCode` — `true` for 5xx/network, `false` for 4xx). Constructor runs `sanitizeError` AND `stripPaymentSignature` on `responseHeaders` defensively; callers (the fetch wrapper) are expected to pass pre-stripped inputs.
3. Each error gets a Symbol marker for cross-package `instanceof` [research §1 "Error Handling Contract"].
4. Unit tests (separate agent): `APICallError.isInstance(err) === true` for each; `err.isRetryable` correct; sentinel-fixture key absent from `JSON.stringify(err)`, `err.message`, `err.stack`, `err.cause`, `err.responseHeaders`, `err.requestBodyValues`, `err.toString()`; `SolvelaSigningError` with base58 in upstream message produces redacted output.

**Deliverables:** `src/errors.ts`, `src/util/redact.ts`, tests.

**Done criteria:** all four error types exported; sentinel-fixture test green; reviewer sign-off.

**Dependencies:** Phase 1.

---

### Phase 3 — Custom fetch wrapper (402 → sign → retry)

**Goal:** Implement HTTP-layer interception: read 402, parse envelope, reserve budget, call signer with AbortSignal, retry with `PAYMENT-SIGNATURE`, finalize budget, sanitize errors. Works for both `doGenerate` and `doStream`.

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=**opus** — architectural crux).
- Reviewer #1 (pre-merge): `oh-my-claudecode:security-reviewer` — key redaction, TLS, retry-bomb, header sanitization.
- Reviewer #2 (pre-merge): `pr-review-toolkit:silent-failure-hunter` — missing awaits, swallowed rejections.
- Tests (separate agent): `oh-my-claudecode:test-engineer`.

**Skills to load:** `vercel-plugin:ai-sdk`, `security-review`, `api-design`, `tdd-workflow`, `m07-concurrency`.

**Work items:**

1. `src/util/parse-402.ts` — parses the gateway envelope `{error: {type, message: JSON}}` per `crates/gateway/src/error.rs:75-80`. Applies the 402-body allowlist from §4.3 (T2-G). Exports `parseGateway402(body: unknown): ParsedPaymentRequired` and `selectAccept(parsed): Accept` which implements the v1 scheme-selection rule from §4.3 — **first `accepts[]` entry with `scheme === 'exact'` AND `asset === USDC` (USDC-SPL)** — and returns the matching entry plus its `amount` as the `cost` for the budget state machine. If no entry matches, `selectAccept` throws `SolvelaPaymentError('no supported payment scheme in accepts[]: v1 requires scheme=exact + asset=USDC')` (P1-4). Rejects unknown envelope shapes with a `SolvelaPaymentError`. The direct `{x402_version, accepts, ...}` shape is NOT supported (T1-B).
2. `src/budget.ts` — `BudgetState` class with `reserve(requestId, cost)`, `finalize(requestId)`, `release(requestId)`. All three are synchronous (single-threaded JS). `reserve` is an atomic check-and-reserve; throws `SolvelaBudgetExceededError` if `available - sumReserved() < cost`. Implements the state machine from §4.3.
3. `src/fetch-wrapper.ts` — `createSolvelaFetch({ signer, budget, logger, maxSignedBodyBytes })` returning a `FetchFunction` matching `(url, init) => Promise<Response>` [research §4.1]. Logic:
   - a. Generate per-call `requestId` (crypto.randomUUID).
   - b. First call: `await baseFetch(url, init)`. Only `resp.status` is inspected. `resp.body` is NOT read on the 200 path (T2-D).
   - c. If `resp.status !== 402`, return as-is.
   - d. If `init.headers?.['PAYMENT-SIGNATURE']` (case-insensitive) is present (T2-F): throw `SolvelaPaymentError('caller supplied PAYMENT-SIGNATURE; refusing to re-sign')`. Do NOT reserve or sign.
   - e. Parse body via `parseGateway402`. On parse failure: throw `SolvelaPaymentError` with sanitised raw-status context.
   - f. Body type + size check (T2-C): `typeof init.body === 'string'` AND `init.body.length <= maxSignedBodyBytes` (default 1_000_000). Throw `SolvelaPaymentError` on violation.
   - g. **Reserve budget atomically.** If reservation fails: throw `SolvelaBudgetExceededError` (`isRetryable: false`). DO NOT call signer.
   - h. Call `signer.signPayment({ paymentRequired, resourceUrl: url, requestBody: init.body, signal: init.signal })`. If signer throws OR abort fires: release reservation, throw `SolvelaSigningError` (or propagate `AbortError`).
   - i. Retry: `await baseFetch(url, { ...init, headers: { ...init.headers, 'PAYMENT-SIGNATURE': header }, signal: init.signal })`. If abort fires between sign and retry (T2-E): release reservation, emit `warn-once('[solvela] aborted mid-retry — signed transaction built but not submitted...')` with NO signature bytes, rethrow `AbortError`.
   - j. On retry success (status 2xx): finalize budget; return response.
   - k. On retry non-2xx but not 402 (e.g., 500 from upstream LLM) — **Option A seam (§4.3)**: release reservation; the wrapper inspects `resp.status` BEFORE returning (it cannot catch downstream `APICallError` because `postJsonToApi` throws upstream of the wrapper's return). The wrapper reads the error-envelope body (non-streaming — safe to buffer), then throws `SolvelaUpstreamError extends APICallError` directly, constructed with sanitized fields only (`statusCode`, redacted `responseBody`, `responseHeaders` with `PAYMENT-SIGNATURE` stripped via `stripPaymentSignature`, `requestBodyValues: undefined`). Throwing directly bypasses `postJsonToApi`'s error conversion so the signed header never reaches a vanilla `APICallError`.
   - l. Retry-bomb guard: if the retry also returns 402 (T1-B + existing Sec-4): release reservation; throw `SolvelaPaymentError("Payment rejected after retry")`. Do NOT loop.
   - m. Audit: log count via a debug counter (Sec-N): each logical `doGenerate`/`doStream` invocation makes at most 2 base-fetch calls on the 402 path. Unit test asserts the count.
4. Wire the returned `FetchFunction` into `provider.ts` by replacing the Phase 2 placeholder.
5. Unit tests (separate agent): all branches (b)-(m), plus the two concurrent-signing scenarios from §4.3 (Unit-6).

**Deliverables:** `src/util/parse-402.ts`, `src/budget.ts`, `src/fetch-wrapper.ts`, updated `src/provider.ts`, tests.

**Done criteria:**
- All branches covered; concurrent-signing tests green.
- Sentinel-key absent from every error surface (verified in Unit-5 + redact tests).
- `pr-review-toolkit:silent-failure-hunter` report: zero findings.
- `oh-my-claudecode:security-reviewer` report: zero findings; TLS guard, header redaction, retry-bomb, budget-finalize-on-success-only all verified.

**Dependencies:** Phase 2, Phase 6.

---

### Phase 4 — Reference adapter implementation (`src/adapters/local.ts`)

**Round 3 retitle:** Phase 4 no longer formalises a signer-abstraction layer (the public `SolvelaWalletAdapter` interface is declared in Phase 2). This phase implements the dev/test reference adapter that ships from the `./adapters/local` sub-export, and wires the fetch wrapper to invoke `settings.wallet.signPayment(...)` directly.

**Goal:** Ship `createLocalWalletAdapter(keypair: Keypair): SolvelaWalletAdapter` from a separate package entry point. Move the SPL `TransferChecked` build + Ed25519 sign logic here. Wire `provider.ts` to invoke the adapter's `signPayment` method directly with no precedence resolution and no runtime-gate.

**Agent assignment (unchanged):**
- Primary: `oh-my-claudecode:executor` (model=**opus** — payment-signing code).
- Reviewer #1: `oh-my-claudecode:security-reviewer` — adapter contract, key handling within the adapter implementation, no leakage from the adapter sub-export.
- Reviewer #2: `pr-review-toolkit:silent-failure-hunter` — silent failure in adapter path.
- Tests (separate agent): `oh-my-claudecode:test-engineer`.

**Skills to load:** `solana-dev`, `security-review`, `tdd-workflow`.

**Work items (Round 3 simplified):**

1. `src/adapters/local.ts` — export `createLocalWalletAdapter(keypair: Keypair): SolvelaWalletAdapter` returning an object with:
   - `label: 'local-test-keypair'`
   - `async signPayment(args)` that builds the SPL `TransferChecked` instruction, signs with Ed25519, returns the base64 PAYMENT-SIGNATURE header value.
   - **Conditional implementation per §3.9:** If the adapter lands in the existing `@solvela/sdk` reuse path (spike-passes), it dynamically imports `@solvela/sdk`'s `createPaymentHeader` (web3.js 1.x is fine for v1.0). If §3.8 Option B triggers and the adapter lands inside the new `@solvela/signer-core`, the implementation MUST use `@solana/kit` + `@solana-program/token` + `@solana/signers` Web Crypto APIs (per §3.9 Round 3 Kit-first conditional), NOT `@solana/web3.js`.
   - File-level JSDoc warning: "DEVELOPMENT AND TESTING ONLY — not for production key material. Production users: implement your own adapter backed by a hardware wallet, MPC signer, or wallet-standard adapter."
   - Throws `SolvelaInvalidConfigError` with install instructions if the underlying peer dep is missing.
2. `provider.ts` (update from Phase 2) — wire the validated `settings.wallet` directly into the fetch wrapper's `signer` parameter. No precedence resolution. No `buildDefaultSigner`. The fetch wrapper invokes `settings.wallet.signPayment({ paymentRequired, resourceUrl, requestBody, signal })` directly.
3. Unit tests (separate agent):
   - Reference-adapter contract: `createLocalWalletAdapter(keypair)` returns an object satisfying `SolvelaWalletAdapter` (label string, `signPayment` callable, AbortSignal propagation).
   - Custom-adapter contract: a hand-rolled `SolvelaWalletAdapter` implementation is invoked correctly by the fetch wrapper (passes through `paymentRequired`, `resourceUrl`, `requestBody`, `signal`); verifies the wrapper does NOT inspect adapter internals.
   - Adapter peer-dep missing → typed `SolvelaInvalidConfigError`.
   - Error message from adapter contains no base58 + no hex (still scrubbed by `SolvelaSigningError` constructor).
   - Same adapter instance called exactly once per logical request (no caching).

**Deliverables:** `src/adapters/local.ts`, updates to `src/provider.ts`, tests. **Removed from prior revision:** `src/signer.ts`, all `OpaquePrivateKey` plumbing, all runtime-gate calls in this phase, `buildDefaultSigner` precedence logic.

**Done criteria:** both reviewers sign off; all tests green; tarball produced from main entry (`./dist/index.js`) does NOT contain any code from `./adapters/local` — verified by `grep` on the main bundle. Tree-shaking ensures production-bundle consumers physically cannot reach key-material code.

**Dependencies:** Phase 3; if §3.8 spike failed, Phase 4 blocks until user approves §3.8 option (Option B mandates `@solana/kit` per §3.9 Round 3 conditional).

---

### Phase 5 — Model registry codegen

**Goal:** Build-time codegen from `config/models.toml` to `src/generated/models.ts`.

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=sonnet).
- Reviewer: `oh-my-claudecode:code-reviewer`.
- Tests (separate agent): `oh-my-claudecode:test-engineer`.

**Skills to load:** `vercel-plugin:ai-sdk`, `tdd-workflow`.

**Work items:**

1. `scripts/generate-models.ts` — reads path resolved from `process.env.SOLVELA_MODELS_TOML || path.resolve(__dirname, '../../../config/models.toml')` (T3). Emits `src/generated/models.ts` with `export type SolvelaModelId = 'openai/gpt-4o' | ... | (string & {})` (Round 3 — Vercel-idiom escape-hatch tail; see §3.5) and `export const MODELS = [...] as const`. The `(string & {})` tail ensures a gateway shipping a new model never produces a TypeScript error for users on an older package version while preserving IDE autocomplete on known models.
2. Hook `npm run generate-models` into `prebuild`.
3. Check `src/generated/` into git (not gitignored) so the build does not require the Rust workspace checkout. CI asserts committed file matches fresh output (drift guard).
4. `src/index.ts` re-exports `SolvelaModelId`.
5. Unit tests: codegen snapshot — run against a fixture TOML and assert output shape.

**Deliverables:** `scripts/generate-models.ts`, `src/generated/models.ts` (committed), tests.

**Done criteria:** `npm run generate-models` idempotent; CI drift guard passes; TS autocomplete on `solvela('...')` shows the full model list.

**Dependencies:** Phase 1. Parallel with Phase 2/3/4/6.

---

### Phase 7 — Unit tests (consolidation + gap-filling)

**Goal:** Cover every module with mocked-fetch unit tests. 85% line / 80% branch floor.

**Agent assignment:**
- Primary: `oh-my-claudecode:test-engineer` (**separate from all implementation authors** per saved memory).
- Reviewer: `oh-my-claudecode:code-reviewer`.

**Skills to load:** `tdd-workflow`, `superpowers:test-driven-development`.

**Work items (one agent invocation per file):**

| # | File | Scope |
|---|---|---|
| Unit-1 | `tests/unit/config.test.ts` | zod validation, env precedence, TLS rejection matrix (dev/prod/edge/test-mode), baseURL `/v1` normalization, missing-wallet rejection. |
| Unit-2 | `tests/unit/parse-402.test.ts` | gateway envelope (the ONLY supported shape); allowlist dropping of extra fields (`internal_trace_id`); malformed body. **Scheme-selection (P1-4):** 402 body with multiple `accepts[]` entries selects the first `scheme: 'exact'` + `asset: USDC` entry and its `amount` flows into the budget state; zero-match 402 throws typed `SolvelaPaymentError`. |
| Unit-3 | `tests/unit/fetch-wrapper.test.ts` | All 12 branches (b)-(m) from Phase 3; exactly-2-fetch-calls count assertion (Sec-N); abort-before-sign vs abort-mid-retry both covered. |
| Unit-4 | `tests/unit/signer.test.ts` | Adapter invocation semantics: AbortSignal propagation to `adapter.signPayment`; single invocation per request (no double-call); `paymentRequired` passthrough verbatim; adapter return value threaded into retry header; `adapter.label` logged, internal state not. |
| Unit-5 | `tests/unit/errors.test.ts` | `APICallError.isInstance`; `isRetryable`; **sentinel-leak battery**: `JSON.stringify(err)`, `err.message`, `err.stack`, `err.cause`, `err.responseHeaders`, `err.requestBodyValues`, `err.toString()` — none contain the sentinel key or sentinel signature. |
| Unit-6 | `tests/unit/budget.test.ts` | Debit-on-success-only; **two concurrent `generateText` calls racing a budget that affords only one → exactly one succeeds, one throws `SolvelaBudgetExceededError`**; cancellation-release (abort mid-retry → reservation released, budget visible to next call). |
| Unit-7 | `tests/unit/provider.test.ts` | Factory shape; `specificationVersion: 'v3'` (Round 3 — V3 channel); URL reaching underlying fetch is exactly `https://api.solvela.ai/v1/chat/completions` (T2-B assertion). |
| Unit-8 | `tests/unit/codegen.test.ts` | models.ts snapshot. **Round 3:** also asserts the emitted union ends with `\| (string & {})` and that a string NOT in the enum (e.g., `"hypothetical-future-model"`) does not produce a TypeScript error (use `// @ts-expect-error` inverted — expect no error). |
| Unit-9 | `tests/unit/adapter-contract.test.ts` | **Round 3 rewrite (was wallet-key):** custom `SolvelaWalletAdapter` implementation invoked correctly by fetch wrapper — `paymentRequired`, `resourceUrl`, `requestBody`, `signal` passed through; wrapper does NOT inspect adapter internals; wrapper logs only `adapter.label`, never any internal state; reference adapter (`createLocalWalletAdapter`) `toJSON()` exposes no key bytes. (Wallet-key/`OpaquePrivateKey` tests are deleted along with the type.) |
| Unit-10 | `tests/unit/redact.test.ts` | `redactBase58` handles 44/55/88-char samples; `redactHex` handles 64+ char hex; `sanitizeError` walks nested `cause`; `stripPaymentSignature` is case-insensitive. |

**Deliverables:** 10 test files. Coverage report.

**Done criteria:** `npm run test -- --coverage` shows >= 85% line, >= 80% branch on `src/**` excluding `src/generated/` and `src/index.ts`. `tsc --noEmit` clean.

**Dependencies:** Phases 2, 3, 4, 5, 6.

---

### Phase 8 — Integration tests (mocked gateway)

**Goal:** End-to-end AI-SDK-level tests using a mocked gateway. Each scenario drives an AI SDK top-level call against `undici.MockAgent`.

**Agent assignment:**
- Primary: `oh-my-claudecode:test-engineer` (separate from implementation).
- Reviewer: `oh-my-claudecode:code-reviewer` + `pr-review-toolkit:silent-failure-hunter`.

**Skills to load:** `vercel-plugin:ai-sdk`, `tdd-workflow`.

**Mock transport:** `undici.MockAgent`. Fresh agent per scenario.

**Work items (one agent invocation per scenario):**

| # | Scenario | Verify |
|---|---|---|
| IT-1 | 402 once, then 200 | `generateText` returns expected `text`; exactly 2 HTTP calls; second carries `PAYMENT-SIGNATURE`. |
| IT-2 | 402 twice (payment rejected on retry) | `generateText` throws `SolvelaPaymentError`; exactly 2 calls (no infinite retry). |
| IT-3 | 402 then SSE stream | `streamText` yields `stream-start` first, then `text-delta`s, then `finish`. |
| IT-4 | Tool call (non-stream) | `generateText` with zod tool returns a `tool-call` content part [research §4]. |
| IT-5 | `generateObject` with schema | Response parsed against zod schema; `response_format` reflects `supportsStructuredOutputs: true` when caller opts in. Separate assertion for `supportsStructuredOutputs: false` (default) — NO `response_format` set. |
| IT-6 | Streaming tool call | `tool-input-start` / `tool-input-delta` / `tool-input-end` / `tool-call` emitted in order. |
| IT-7 | Network error on retry | Sanitized `APICallError` with `isRetryable: true` (network path); NO `PAYMENT-SIGNATURE` in `responseHeaders`/`requestBodyValues` (T1-C); 402-retry does not fire on 500s. |
| IT-8 | Budget exceeded | `generateText` throws `SolvelaBudgetExceededError` before any sign call; no 402 retry fetch made. |
| IT-9 | User-supplied `signPayment` callback | Callback invoked with correct `paymentRequired` + `signal`; returned header reaches the retry. |
| IT-10 | Abort signal mid-retry | `AbortController.abort()` cancels cleanly; reservation released; one-time warn emitted (no signature bytes). |
| IT-11 | **Mid-stream retry not attempted** (T2-D) | 200 response followed by a stream-error chunk surfaces as SDK stream error; wrapper is not re-entered. |
| IT-12 | **Caller-supplied PAYMENT-SIGNATURE on initial request** (T2-F) | Initial request carries header; gateway returns 402; wrapper surfaces `SolvelaPaymentError` directly, does NOT re-sign. |
| IT-13 | **Sanitized upstream 500 on retry leg** (T1-C, Option A seam) | First request returns 402; signed retry returns 500 from upstream LLM. Assert the thrown error is `SolvelaUpstreamError` (NOT a vanilla `APICallError` from `postJsonToApi`); `statusCode === 500`; `responseHeaders` does NOT contain `PAYMENT-SIGNATURE` (any case); `responseBody` is redacted; `requestBodyValues === undefined`; full sentinel battery across `message`/`stack`/`cause`/`toString`/`JSON.stringify` shows no sentinel signature. Also covers same-session follow-up call where upstream 500s after the first retry succeeded. |

**Deliverables:** `tests/integration/mock-gateway.ts` + 13 scenario files.

**Done criteria:** 13/13 green; no flakiness over 20 consecutive runs (CI `--retry 0`).

**Dependencies:** Phase 7.

---

### Phase 9 — Docs

**Goal:** README + Fumadocs MDX page that a new user can follow to make their first `generateText` call in < 5 minutes.

**Agent assignment:**
- Primary: `oh-my-claudecode:writer`.
- Reviewer: `oh-my-claudecode:code-reviewer` (checks code samples compile).
- Hallway tester (T3): one external person (user-recruited) follows README cold.

**Skills to load:** `vercel-plugin:ai-sdk`.

**Auto-accept posture:** OFF (docs can contain secrets in code examples).

**Work items (Round 3 ownership inversion per §3.7):**

1. **`README.md` (minimal — npm-page entry point):**
   - Install (one command).
   - 10-line Quick start: `createSolvelaProvider({ baseURL, wallet: createLocalWalletAdapter(keypair) })` + one `generateText` call.
   - Link to canonical docs (`docs.solvela.ai/sdks/ai-sdk` once migration runs; `dashboard/content/docs/sdks/ai-sdk.mdx` is the source of truth today).
   - Link to error reference + adapter authoring guide on docs site.
   - Brief note: "For full features (streaming, tools, structured output, session budgets, observability, custom adapter authoring), see the docs site."

   **Round 3:** README is intentionally short. The canonical reference lives in MDX (WI-3). README does NOT duplicate the full feature reference. Stripe / Supabase / every Vercel community provider follow this pattern.

2. `examples/` — one Next.js 16 app route (custom-adapter), one Node CLI script (`createLocalWalletAdapter`). Both reference the published package but support `npm link` in dev. **`examples/` ships WITH the package (P1-6)** — explicitly included in the `package.json` `files` array (§5.1); kept small and benign. No `.env*` or key material lands there; the docs-leak validator (WI-5) and Phase 10 Sec-9 inspection also scan `examples/`.

3. **`dashboard/content/docs/sdks/ai-sdk.mdx` (canonical reference — Round 3 ownership inversion):** authored independently from README, NOT synced from it. Sections:
   - Overview + install.
   - Authentication — adapter interface (`SolvelaWalletAdapter`); when to use the reference `createLocalWalletAdapter` vs implementing your own; production guidance (hardware wallets, MPC, wallet-standard).
   - **Adapter authoring guide** — full walkthrough of implementing a `SolvelaWalletAdapter` for production (hardware wallet, MPC, remote signer). Round 3 addition.
   - Streaming.
   - Tool calls.
   - Structured output (`generateObject`) — documents `supportsStructuredOutputs` default false + opt-in.
   - Session budgets — explains the reservation/debit semantics so users understand concurrent-request behaviour.
   - Error reference — table of each Solvela error class, when thrown, `isRetryable`.
   - **Observability integration** — Sentry `beforeSend` snippet that scrubs `PAYMENT-SIGNATURE` from any captured error (T1-C fix):
     ```ts
     // sentry.client.config.ts or server config
     Sentry.init({
       beforeSend(event) {
         // Defense-in-depth; the provider already sanitizes errors, but if a caller
         // catches and re-throws, this ensures signatures never reach Sentry.
         if (event.request?.headers) delete event.request.headers['PAYMENT-SIGNATURE'];
         if (event.request?.headers) delete event.request.headers['payment-signature'];
         return event;
       }
     });
     ```
   - Migration from `openai` SDK — one-line import swap; `apiKey` replaced by `wallet: SolvelaWalletAdapter`.
   - Environment variables table.
   - Troubleshooting / FAQ.

   Mirrors `dashboard/content/docs/sdks/typescript.mdx` structure. Note at top: "This page will be picked up by the docs.solvela.ai migration when it runs." Different audiences from README, different depth.

4. Migration-from-OpenAI working code sample lives in the MDX (WI-3); README links to it.
5. **Docs key-leak validator** (T3/M6) — pre-commit or CI regex that rejects strings matching base58-pubkey shape (44-88 chars of `[1-9A-HJ-NP-Za-km-z]`) in BOTH `README.md` and `dashboard/content/docs/sdks/ai-sdk.mdx`. Run as `scripts/check-docs-for-leaked-keys.ts`.
6. **Hallway test** — user recruits one external tester; tester follows README + linked MDX cold; first-sticky-point reported back and a revision PR addresses it before Phase 10.

**Deliverables:** README + examples/ + Fumadocs MDX + docs-leak validator + hallway-test report.

**Done criteria:**
- Every code block in README and MDX `tsc`-checked by `docs/verify-snippets.ts`.
- Key-leak validator passes.
- **`dashboard/content/docs/sdks/ai-sdk.mdx` is committed and renders cleanly in the local Fumadocs preview** — Round 3 ship gate (canonical reference, not optional).
- Hallway tester reaches successful `generateText` call OR their first-sticky-point is captured and fixed in this phase.

**Dependencies:** Phase 8.

---

### Phase 10 — Publish + community-provider PR

**Goal:** `0.1.0` on npm (with provenance) and a PR to `vercel/ai` adding the community-provider page. **Runs AFTER Phase 11 per recommendation below.**

**Agent assignment:**
- Primary: `oh-my-claudecode:executor` (model=sonnet) — mechanical, CI-driven.
- Reviewer #1: `oh-my-claudecode:security-reviewer` — final secret scan pre-publish.
- Reviewer #2: `oh-my-claudecode:verifier` — completion evidence.

**Skills to load:** `vercel-plugin:ai-sdk`, `deployment-patterns`, `security-review`.

**Auto-accept posture:** OFF. Every publish step requires explicit approval.

**Work items:**

1. `npm pack --dry-run` inspection: no `.env*`, no `*.key`, no `node_modules/`, no `tests/`, no `scripts/`. The tarball contains: `dist/`, `examples/` (P1-6 — shipped per §5.1 `files` array), `README.md`, `LICENSE`, `package.json`. Additional sub-check: `examples/` contains no `.env*`, no `*.key`, no hard-coded base58 keys (regex from the docs-leak validator, Phase 9 WI-5).
2. `npm audit --production` — HIGH/CRITICAL blocks publish (T2-I).
3. `npm ls --all --json` — asserts no unexpected transitive postinstall scripts.
4. **Signed git tag**: `git tag -s sdks/ai-sdk-provider/v0.1.0 -m "..."` (T2-J).
5. **Publish from CI only** (T2-J): GitHub Actions workflow triggered on tag push. Steps:
   - `npm ci` (no postinstall scripts, enforced by guard).
   - `npm run build`.
   - `npm run test` (unit + integration).
   - `npm run size` — bundle-size guard.
   - `npm publish --access public --provenance` — provenance via OIDC (`id-token: write`).
   - Verify published tarball SHA matches the CI-built tarball.
6. Fork `vercel/ai`, branch `community-provider-solvela`, add `content/docs/providers/03-community-providers/solvela.md` using content from the README.
7. Open PR. Link to Solvela site, package on npm, and this plan for reviewer context.
8. Announce: dashboard changelog, Discord, X — copy drafted by `oh-my-claudecode:writer` in parallel; execution gated by user sign-off.
9. **Rollback procedure** (T3) documented: if a shipped 0.1.0 is broken, run `npm deprecate @solvela/ai-sdk-provider@0.1.0 "broken release, use 0.1.1"`; publish 0.1.1 fix; never `npm unpublish` (retention policy + consumer disruption).

**Deliverables:** npm package (with provenance attestation), signed git tag, PR URL, documented rollback procedure.

**Done criteria:**
- `npm view @solvela/ai-sdk-provider` shows 0.1.0 with provenance badge.
- PR is open (merge not required for plan-complete).
- Changelog entry live.
- Rollback runbook in `sdks/ai-sdk-provider/RELEASE.md`.

**Dependencies:** Phases 1-9 AND Phase 11.

---

### Phase 11 — Live-gateway smoke test (RECOMMENDED BEFORE Phase 10)

**T2-K resolution:** the prior plan's open question "before or after Phase 10?" is resolved — BEFORE. Shipping a broken package and then discovering a real-network regression is strictly worse than catching the regression pre-publish.

**Goal:** Validate the full stack against a running Solvela gateway on devnet.

**Agent assignment:** `oh-my-claudecode:verifier`. User-gated.

**Skills to load:** `solana-dev`, `docker-patterns`.

**Work items:**

1. `docker compose up -d` per CLAUDE.md "Local dev stack".
2. Configure `SOLVELA_SOLANA_RPC_URL` and the devnet keypair from §2.
3. `generateText({ model: solvela('gpt-4o'), prompt: 'Echo: hello' })` — succeeds with real 402 → signed tx → 200.
4. `streamText` same pattern.
5. **Abort-mid-retry scenario** (T2-E): fire `AbortController.abort()` immediately after the signer returns; verify no double-spend observable on-chain via `getSignatureStatuses` / block explorer; verify the warn is emitted without any signature bytes in its output.
6. Check gateway logs show the Solana signature, not a stub.

**Deliverables:** smoke-test transcript attached to completion evidence; devnet tx signatures recorded.

**Done criteria:** one successful real payment; abort scenario verified no double-spend.

**Dependencies:** Phases 1-9.

---

## 7. Test strategy

### 7.1 Unit (Phase 7)

- Framework: `vitest`.
- Coverage floor: 85% line, 80% branch across `src/**` excluding `src/generated/` and `src/index.ts`.
- Modules targeted: config, parse-402, fetch-wrapper, wallet-adapter, adapters/local, errors, budget, provider, redact, codegen snapshot.
- Every signer failure, every config rejection, every budget edge case, every sentinel-leak check.

### 7.2 Integration (Phase 8)

- Framework: `vitest` + `undici.MockAgent`.
- 13 scenarios listed in §6 Phase 8.
- No live network, no live Solana, no live gateway.

### 7.3 End-to-end (Phase 11)

- Against local `docker compose` gateway with funded devnet wallet.
- Gates Phase 10 publish (T2-K).
- Not part of CI default (requires secrets + funded wallet).

### 7.4 Deliberate-mode test plan additions (T1 security)

- **Sentinel-leak battery** (§8 Sec-2/Sec-3): a fixture non-funded keypair `tests/fixtures/sentinel-key.base58` is used in every test that constructs a signer. Every `catch (err)` branch runs the 7-surface assertion (`JSON.stringify`, `message`, `stack`, `cause`, `responseHeaders`, `requestBodyValues`, `toString`).
- **Observability scrub test** — an intercepted `console.error`/`console.warn` and a mock Sentry `beforeSend` verify no sentinel value reaches either.
- **Concurrency race test** (§4.3 budget) — 50 concurrent `generateText` invocations against a mock gateway that affords 25 payments; assert exactly 25 succeed, 25 throw `SolvelaBudgetExceededError`, and the total signed-header count reaching the mock is exactly 25.

### 7.5 Pre-mortem (deliberate mode, T1 scope)

Three top failure scenarios considered in design:

1. **Production Sentry captures a signed transaction.** Prevented by: (a) fetch-wrapper error sanitizer strips `PAYMENT-SIGNATURE` before rethrow; (b) Solvela error constructors run `sanitizeError`; (c) README Sentry `beforeSend` snippet as defense-in-depth; (d) IT-7, IT-13, Unit-5 all assert sentinel absence.
2. **Budget overspend under concurrency.** Prevented by: (a) synchronous reserve-and-check critical section; (b) finalize-on-success-only, release-on-every-error-path; (c) Unit-6 race + cancellation tests; (d) S12 success criterion.
3. **Browser/Edge consumer exfiltrates keys via a naïve adapter.** Prevented by: (a) `SolvelaWalletAdapter` is an opaque interface — the provider never sees key bytes directly; (b) `createLocalWalletAdapter` ships from a separate sub-export (`@solvela/ai-sdk-provider/adapters/local`) — browser bundlers that only import the main entry tree-shake it away; (c) README leads with "production: implement your own adapter" and relegates `LocalWalletAdapter` to dev/test; (d) adapter labels are logged, internal state is not; (e) Unit-9 adapter-contract tests verify the provider treats the adapter as opaque.

### 7.6 Evidence of completion (per phase)

- `vitest run --coverage` output attached.
- `tsc --noEmit` output attached.
- `npm run build` output attached.
- For Phases 3, 4, 6, 8, 10: reviewer sign-off logs attached.
- For Phase 11: devnet tx signatures + block-explorer screenshots.
- For Phase 9: hallway-tester report.

---

## 8. Security review checklist

Executed via `oh-my-claudecode:security-reviewer` (Phase 3, 4, 10 pre-merge; Phase 10 pre-publish). Follows `~/.claude/rules/common/security.md`.

| # | Check | How verified |
|---|---|---|
| Sec-1 | Non-HTTPS `baseURL` is rejected at `createSolvelaProvider` time unless `allowInsecureBaseURL: true` or `SOLVELA_AI_SDK_PROVIDER_TEST_MODE` is set. | Unit-1; reviewer confirms env-gates doc'd dev-only. |
| Sec-2 | Private-key bytes never appear in `err.message`, `err.responseBody`, `err.responseHeaders`, `err.requestBodyValues`, `err.stack`, `err.cause`, `err.toString()`, `JSON.stringify(err)`, or any `console.*`/`tracing`-equivalent emission. | Sentinel-fixture battery across Unit-5, Unit-9, Unit-10, IT-13. |
| Sec-3 | **No Solvela-produced error, log, stack, or trace may contain the value of the `PAYMENT-SIGNATURE` header.** Applies to `APICallError.requestBodyValues`, `responseHeaders`, `responseBody`, `message`, `stack`, `cause`, and any `console.*` emission from Solvela's code. (T1-C fix — explicit clause.) The seam (§4.3 Option A): the wrapper inspects non-2xx retry status BEFORE returning and throws `SolvelaUpstreamError` directly with sanitized fields, bypassing `postJsonToApi`'s error conversion so the signed header never lands in a vanilla `APICallError`. | Fetch-wrapper throws `SolvelaUpstreamError` on non-2xx retry; every Solvela error constructor runs `sanitizeError`; Unit-3, Unit-10, IT-7, IT-13. |
| Sec-4 | Retry happens **at most once**. No recursive / unbounded retry on 402-after-retry. | Unit-3 branch (l); IT-2. |
| Sec-5 | Budget is debited only after retry 2xx. Failed retries do NOT consume budget. Reservations released on every error path (including abort). | Unit-6 (budget tests); concurrent race test. |
| Sec-6 | `signPayment` callback runs with the raw parsed 402 body; provider does NOT mutate before handing off. | Unit-4. |
| Sec-7 | Signatures are never reused across requests. Each 402 triggers a fresh signing call. Mock `signPayment` assert called once per request. | Unit-4, IT-1. |
| Sec-8 | Dynamic `import('@solvela/sdk')` path does not allow injection via env-derived strings. | Static analysis; reviewer confirm. |
| Sec-9 | `npm pack --dry-run` contains no `.env*`, no `*.key`, no `node_modules/`, no `tests/`, no `scripts/` — only `dist/` + README + LICENSE + `package.json`. | Phase 10 work item 1. |
| Sec-10 | `package.json` does NOT include `@solana/web3.js` as hard dep — optional peer. | Phase 1 reviewer. |
| Sec-11 | `SOLVELA_ALLOW_INSECURE_BASE_URL` refuses to apply in `NODE_ENV === 'production'`, `VERCEL_ENV === 'production'`, and on Vercel Edge runtime. (T2-H) | Unit-1 matrix. |
| Sec-12 | Caller-supplied `PAYMENT-SIGNATURE` does NOT trigger re-sign on 402 — surfaces as `SolvelaPaymentError`. (T2-F) | IT-12. |
| Sec-13 | **`SolvelaWalletAdapter` contract (Round 3 rewrite — was runtime-gate):** adapters are responsible for their own key-material handling; the provider treats the adapter as opaque; the provider logs only `adapter.label`, never any internal state. Reference adapter ships from `./adapters/local` sub-export marked dev/test only; production bundles importing only the main entry never bundle local-keypair code via tree-shaking. | Unit-9 adapter-contract test; reviewer inspects `exports` map; `grep` on main bundle confirms `./adapters/local` code absent. |
| Sec-14 | 402 body allowlist — extra fields (e.g., `internal_trace_id`) are dropped before reaching `err.responseBody`. (T2-G) | Unit-2. |
| Sec-15 | `SolvelaSigningError` constructor scrubs base58 + hex from wrapped upstream message. (M3) | Unit-5. |
| Sec-16 | Request body size cap (default 1 MB) rejected before signer invocation. Non-string body also rejected. (T2-C) | Unit-3. |
| Sec-17 | Abort-mid-retry emits warn (no signature bytes) AND releases budget reservation. (T2-E) | Unit-3, Unit-6, IT-10, Phase 11 scenario 5. |
| Sec-18 | **Removed in Round 3** — adapter interface eliminates `signPayment`/`wallet` precedence. `wallet` is the single required adapter; no conflict surface exists. The `headers`-filter warn-once for caller-supplied `PAYMENT-SIGNATURE` is preserved (Sec-12). | n/a — adapter pattern eliminates the conflict scenario. |
| Sec-19 | Supply-chain hygiene: exact-pin `@ai-sdk/openai-compatible`; no `postinstall`/`preinstall`/`prepublish` scripts; `npm audit --production` HIGH/CRITICAL blocks publish; `npm ls --all` drift guard. (T2-I) | Phase 1 work item 2 + CI workflow + Phase 10. |
| Sec-20 | npm publish: 2FA automation token; `--provenance` via GitHub OIDC; signed git tag; CI-only publish; verify tarball SHA. (T2-J) | Phase 10 work items 1-5. |
| Sec-21 | Exactly 2 base-fetch calls per logical invocation on the 402 path; 1 on the 200 path; 0 extra retries. | Unit-3 counter assertion; IT-1, IT-2. |
| Sec-22 | `SOLVELA_AI_SDK_PROVIDER_TEST_MODE` honoured ONLY when `NODE_ENV === 'test'` AND baseURL hostname is localhost/127.0.0.1. (M5) | Unit-1. |
| Sec-23 | README/MDX key-leak validator: regex rejects base58-shape keys in docs. (M6) | `scripts/check-docs-for-leaked-keys.ts` in pre-commit + CI. |

Matched against `~/.claude/rules/common/security.md` "Mandatory Security Checks" list.

---

## 9. Release checklist

- [ ] CHANGELOG.md entry at `sdks/ai-sdk-provider/CHANGELOG.md` with `0.1.0` scope.
- [ ] Root `CHANGELOG.md` entry (repo-level): "First external plugin shipped".
- [ ] Signed git tag `sdks/ai-sdk-provider/v0.1.0` (`git tag -s`).
- [ ] `npm publish --access public --provenance` from CI only.
- [ ] `npm view @solvela/ai-sdk-provider` confirms version + provenance.
- [ ] `vercel/ai` community-provider PR opened.
- [ ] Dashboard changelog updated.
- [ ] Discord / X announcement copy reviewed and posted.
- [ ] `HANDOFF.md` updated.
- [ ] Fumadocs page live at `dashboard/content/docs/sdks/ai-sdk.mdx` (docs.solvela.ai picks up separately).

### Versioning policy (T3/P1-6)

- **0.x releases**: breaking changes allowed on minor bumps (0.1 → 0.2) while AI SDK v7 is beta. Patches (0.1.0 → 0.1.1) reserved for bug-fix-only, no API changes.
- **0.2 triggers**: any of: (a) AI SDK v7 stable releases; (b) `@solvela/sdk` breaking changes we must follow; (c) §3.8 resolution if the spike forces a new dependency; (d) default-signer migration per §3.9.
- **V3 → V4 upgrade gate (Round 3):** the `specificationVersion: 'v3'` → `'v4'` literal change ships in the same release that bumps the `ai` peer to `^7.0.0`. Single PR, single line in the provider, single peer-dep range bump. Tracked as a 0.2 trigger.
- **Repo-split decision review (Round 3):** the v1.0 milestone explicitly includes a review of §3.3 Option A (in-repo) vs Option B (separate repo). Industry precedent (Stripe per-language repos, Supabase JS client standalone, every Vercel community provider as a separate repo) favors separation once the SDK needs an independent release cadence against Vercel AI SDK beta cycles. If separate-cadence pressure has materialized by v1.0, `git subtree split` extracts `sdks/ai-sdk-provider/` with full history.
- **1.0 ships** when: `ai` v7 stable has shipped AND we have run in production for ≥ 2 weeks with no P0 bug AND the API surface has not changed in 4 weeks AND the repo-split decision has been made.

---

## 10. Process rules

Restated so every execution agent sees them.

1. **Main agent does not code.** The main Claude Code session coordinates. Every line of TypeScript, every test, every README line is written by a delegated subagent. Saved memory `feedback_delegation.md`.
2. **All code written by specialist agents** named in each phase:
   - Implementation: `oh-my-claudecode:executor` (`model=sonnet` default; `model=opus` for Phase 3 and Phase 4).
   - Tests: `oh-my-claudecode:test-engineer` — ALWAYS a different agent invocation than implementation author (saved memory `feedback_test_author_separation.md`). If that agent does not exist in the OMC catalog, substitute `oh-my-claudecode:executor` with an explicit test-only prompt in a separate invocation.
   - Security: `oh-my-claudecode:security-reviewer` (Phases 3, 4, 6, 10). **Fallback (P1-5):** If `oh-my-claudecode:security-reviewer` is not available in the OMC catalog at execution time, substitute `oh-my-claudecode:code-reviewer` with an explicit security-only prompt covering the Sec-1 through Sec-23 checklist items applicable to the phase under review. See §5.4 for per-phase Sec-item mapping.
   - Silent-failure hunting: `pr-review-toolkit:silent-failure-hunter` (Phases 3, 4, 8).
   - Code review: `oh-my-claudecode:code-reviewer` (end of every phase).
   - Verification: `oh-my-claudecode:verifier` (completion evidence, Phase 11).
   - Docs: `oh-my-claudecode:writer` (Phase 9, announcements).
3. **No general-purpose agents.** Every invocation targets one specialist for one work item.
4. **Research-first.** This plan cites the research report for every technical claim. Executors do NOT re-research; consult `document-specialist` only if a citation fails to reproduce.
5. **Test authors separate from implementation authors.** Applied per saved memory.
6. **Quality over speed.** If a phase runs long, extend it — do not compress.
7. **Verification before completion.** Every "Done criteria" must have evidence attached.
8. **Commits follow `~/.claude/rules/common/git-workflow.md`** — conventional commits; type `feat`; one commit per phase minimum.
9. **No attribution footer** — disabled globally.
10. **Cancellation:** if blocked, surface via `<remember>` and pause. Do not invent workarounds.
11. **Follow-up tasks tracked in `.omc/plans/open-questions.md`** — includes: Rust-side contract test asserting gateway `error.rs` output still matches `tests/fixtures/402-envelope.json`; revisit §3.9 when `@solvela/sdk` migrates to `@solana/kit`.

---

## 11. Risk register

| # | Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|---|
| R1 | `ai` v7 stable releases during implementation, shifting V3/V4 calculus. | Low (Round 3 — V3 is stable channel, no peer-dep beta warnings) | Low | Target V3 per §3.1 (Round 3 reversal); single-line literal+peer bump in the same release that bumps `ai` peer to `^7.0.0`. Documented as 0.2 trigger in §9. |
| R2 | `@ai-sdk/openai-compatible` minor version drift. | Low (exact-pin) | High | Exact-pin in beta; CHANGELOG watch; pinned-version CI job. |
| R3 | Gateway 402 body shape changes silently. | Low | High | Phase 1 contract fixture in both repos; CI contract test on both sides. |
| R4 | Signer ergonomics bad for external users (hardware wallets). | Medium | Medium | Docs as P0; two worked examples (callback + wallet); hybrid API in §3.4; hallway test in Phase 9. |
| R5 | `@solvela` npm org missing / no publish rights. | Low | High | §2 item #1 confirms before execution; fallback §3.2 Option B. |
| R6 | **ESM/CJS interop breaks under Next.js / Turbopack (T1-D).** | **High for the Turbopack scenario specifically (Round 3 — open Next.js issue #78267 documents Turbopack failing on CJS packages with runtime `require()` inside function bodies);** Medium overall. | High | Phase 1 spike pass-bar (Round 3) requires success in BOTH bare Node 20 ESM AND `next build` Turbopack. §3.8 options; planner recommends Option B (`@solvela/signer-core` built on `@solana/kit` per §3.9 Round 3 conditional). |
| R7 | **Concurrent-signing race under sessionBudget (T1-A).** | Medium without fix | High | Reservation state machine; Unit-6 race test; S12 success criterion. |
| R8 | **PAYMENT-SIGNATURE leakage into observability (T1-C).** | High without fix | Critical | Error sanitizer; README Sentry snippet; IT-7, IT-13; Unit-5 sentinel battery. |
| R9 | **Adapter implementation quality (Round 3 revision — was wallet runtime exposure).** Risk shifts from "SDK misuses keys" (eliminated by adapter pattern) to "user ships a bad adapter implementation that leaks keys client-side." | Medium without mitigation | High | Reference `createLocalWalletAdapter` ships from a separate entry point with explicit dev/test-only warnings; docs MDX includes a full adapter authoring guide with security checklist; production guidance directs users to hardware-wallet / MPC / wallet-standard adapters; `exports` map ensures local-keypair code never reaches main-entry consumers via tree-shaking. |
| R10 | **Web3.js-based `@solvela/signer-core` would immediately need rewriting (Round 3).** Probability **escalates from Medium to High when the §3.8 Option B path is triggered.** | High when triggered | Medium | §3.9 Round 3 conditional MANDATES `@solana/kit` for any new `@solvela/signer-core` package. Reuse-existing-`@solvela/sdk` (web3.js 1.x) is acceptable for v1.0 only on the spike-passes path. |
| R11 | **Abort-mid-retry double-spend (T2-E).** | Low | Medium | `AbortSignal` propagation; warn; Phase 11 scenario 5. |
| R12 | Supply-chain compromise via transitive deps or malicious postinstall. | Low | High | Exact-pin beta; no install scripts; `npm audit`; `npm ls --all` drift guard; `--provenance`. |
| R13 | npm publish from laptop → account compromise. | Low | High | CI-only publish; 2FA automation token; signed tags. |

Lower-probability risks tracked: Node 18 EOL mid-plan (bump to 20); `undici.MockAgent` API change; `@solana/web3.js` 2.0 breaking change (R6/R12 overlap).

---

## 12. Timeline / sequencing

No dates committed. **Revised DAG (T2-A + T2-K):**

```
Phase 1 (scaffolding + contract fixture + spike) ───┐
                                                    ├─► Phase 6 (errors) ─► Phase 2 (factory) ─► Phase 3 (fetch wrapper) ─► Phase 4 (signer)
Phase 5 (codegen) ──────────────────────────────────┘                                                                       │
                                                                                                                            ▼
                                                                                                                  Phase 7 (unit tests)
                                                                                                                            │
                                                                                                                            ▼
                                                                                                                  Phase 8 (integration)
                                                                                                                            │
                                                                                                                            ▼
                                                                                                                     Phase 9 (docs)
                                                                                                                            │
                                                                                                                            ▼
                                                                                                                 Phase 11 (live smoke)
                                                                                                                            │
                                                                                                                            ▼
                                                                                                                    Phase 10 (publish)
```

**Parallelisable:**
- Phase 1 and Phase 5 in parallel.
- Phase 6 after Phase 1; Phase 2 after Phase 6.
- Phase 7 unit-test subagents in parallel (one per test file).
- Phase 8 integration-test subagents in parallel (one per scenario).
- Phase 9 docs can begin after Phase 6; finalised after Phase 8.

**Strictly serial:**
- Phase 1 → Phase 6 → Phase 2 → Phase 3 → Phase 4.
- Phase 8 → Phase 9.
- Phase 9 → Phase 11 → Phase 10.

**Spike gate:** Phase 4 blocks until the Phase 1 spike result is resolved (passes, or user approves §3.8 option).

---

## 13. Template extraction (for future plugin plans)

This plan is the template for subsequent Solvela plugin plans (LangChain, drop-in OpenAI shim, Python AI SDK, Go AI SDK, etc.). The breakdown of what to reuse and what to redesign:

### Generalizable across future plugin plans (lift-and-adapt)

- **402-interception seam as a fetch wrapper (not framework internals).** The wrapper pattern — `baseFetch → 402 → parse envelope → reserve budget → sign → retry → finalize` — is framework-agnostic. LangChain, OpenAI-shim, and custom agents all get the same wrapper.
- **Signer adapter interface — typed `WalletAdapter` pattern (Round 3).** §3.4 Option B is the right default for every framework. Matches Coinbase x402, Solana Wallet Adapter, AWS SigV4, wagmi/viem. LangChain plan will reuse this exact `SolvelaWalletAdapter` interface (renamed for the framework's namespace).
- **Error taxonomy** — `PaymentError`, `BudgetExceededError`, `SigningError`, `InvalidConfigError`. Only the parent class (`APICallError` here, something else for LangChain) changes.
- **Model registry codegen from `config/models.toml`.** Identical pattern per language.
- **Retry-bomb guard** — at most one retry on 402-after-retry.
- **Budget reservation/debit state machine** (§4.3 + T1-A resolution) — portable.
- **Security checklist** — Sec-1 through Sec-23 port ~90% verbatim; only error parent class differs.
- **Adapter sub-export pattern (Round 3 — replaces opaque-key + runtime gating).** Reference dev/test adapter ships from a separate package entry point so production bundles physically cannot reach key-material code via tree-shaking. Eliminates the need for opaque-key types and runtime-gates. Conceptually reusable in Python (separate module import path) and Go (separate package).
- **npm publish hygiene** (T2-J) — applies to every npm package.

### Vercel-AI-SDK-specific (must be re-designed per framework)

- `createOpenAICompatible` wrapping — LangChain has its own `BaseChatModel` class hierarchy; OpenAI-shim has none (pure HTTP).
- `specificationVersion` V3/V4 strategy — AI-SDK only.
- Stream-part ordering rules (`stream-start`, `text-delta`, `finish`) — AI-SDK-specific.
- `supportedUrls` multimodal mapping — AI-SDK-specific.
- `supportsStructuredOutputs` flag — AI-SDK-specific.
- Community-provider registry PR to `vercel/ai` — platform-specific.

### Hybrid (same principle, different implementation per plugin)

- Peer-dep range strategy (peer vs regular, caret vs exact-pin, beta handling).
- Package naming convention (scope vs unscoped, framework-specific naming conventions).
- Docs destination (per-framework docs site or per-package README).
- CI matrix (Node 18+20 here; different for Python, Go, Rust).

**Recommendation:** Before authoring the next plugin plan (LangChain), extract the Generalizable items into `docs/plugin-template.md` so the next plan applies the template rather than re-deriving. The Generalizable items listed above are the scaffold; the per-plan sections layer the framework-specific detail on top.

---

## Citation key

`[research §X.Y]` refers to `/home/kennethdixon/projects/Solvela/docs/superpowers/research/2026-04-16-vercel-ai-sdk-provider-research.md`, section X.Y. Every non-trivial technical claim is cited to that file or to a specific file/line in the existing repo (`sdks/typescript/src/x402.ts`, `crates/gateway/src/error.rs`, `crates/gateway/src/middleware/x402.rs`, `CLAUDE.md`, etc.).

---

## Appendix A — Review Round 1: Findings Addressed

**Summary: 7 Tier 1 addressed, 13 Tier 2 addressed, 14 Tier 3 addressed, 0 deferred.**

(Tier 3 polish items that required substantial phase rework were either inlined where cost was low, or preserved as notes — none deferred to post-0.1.0.)

| Finding | Source | Addressed in |
|---|---|---|
| T1-A Concurrent signing / budget TOCTOU race | Architect P1-2, Critic P0-1 | §4.3 Budget state machine; §6 Phase 3 work item 2 + 3.g/3.j; §6 Phase 7 Unit-6 (race + cancellation); §7.4 concurrency battery; §11 R7; S12 success criterion |
| T1-B Gateway 402 envelope unverified / wrong shape supported | Architect P0-3, Critic P0-2 | §4.3 "Gateway 402 envelope"; §6 Phase 1 work item 8 (cross-repo contract fixture); §6 Phase 3 work item 1 (envelope-only parser); §10 item 11 (follow-up Rust contract test); §11 R3 |
| T1-B PAYMENT-SIGNATURE header name authoritative source | Architect P0-3, Critic | §4.3 "Signer invocation + PAYMENT-SIGNATURE header name" — cites `crates/gateway/src/middleware/x402.rs` L38 (case-insensitive `payment-signature`) and the canonical uppercase emission |
| T1-C PAYMENT-SIGNATURE leakage via APICallError into Sentry/OTel | Security C3, Architect, Critic P1-7 | §4.3 "Error surface sanitization"; §6 Phase 3 work item 3.k (error catch + rewrap); §6 Phase 6 (errors.ts `sanitizeError`); §6 Phase 9 Sentry `beforeSend` snippet; §7.4 observability scrub; §8 Sec-3 explicit clause; IT-7 + IT-13 + Unit-5 sentinel battery; §11 R8 |
| T1-D ESM-vs-CJS interop with `@solvela/sdk` | Architect P0-1, Critic P1-4 | §3.8 NEW Open Decision with Options A/B/C; §6 Phase 1 work item 9 (ESM/CJS spike); §6 Phase 4 dependency on spike result; §11 R6 |
| T1-E `wallet.privateKey: string` leak vector | Security C1 | §3.4 `OpaquePrivateKey` branded type (toJSON + util.inspect); §6 Phase 2 work item 2 (`wallet-key.ts`); §7 Unit-9 JSON.stringify + util.inspect tests; §11 R10; documents V8 string-immutability limitation |
| T1-F Wallet path not runtime-gated | Security C2 | §4.1 `assertNodeRuntime` semantics; §6 Phase 2 work item 1 (`runtime-gate.ts`); §6 Phase 2 work item 4 (provider construction gate); §6 Phase 4 work item 1 (defensive gate in signer); §6 Phase 9 README lead (callback-first, wallet as Node-only section); §8 Sec-13; §11 R9; `package.json` `exports` `./node` entry |
| T1-G Broken docs path `/rcr-docs-site/` | Critic P0-3 | §3.7 revised to `dashboard/content/docs/sdks/ai-sdk.mdx`; §6 Phase 9 work item 3; §9 release checklist |
| T2-A Phase 6 too late (referenced before declared) | Architect P1-1 | §6 phase order revised — Phase 6 now runs BEFORE Phase 3; §12 DAG updated |
| T2-B baseURL `/v1` prefix ambiguity | Architect P0-2 | §4.3 "baseURL handling"; §6 Phase 2 work item 3 (normalization); Unit-7 exact-URL assertion; §4.1 API surface note |
| T2-C Signer body-type assumption (multimodal) | Architect P0-5, Security M1 | §4.3 "Request body size cap" (tightened scope: string-only + 1MB cap); §6 Phase 3 work item 3.f; §8 Sec-16 |
| T2-D Streaming retry edge-case invariants | Architect P0-4, Critic P1-7 | §4.3 "Fetch-wrapper invariants" (no body read on 200; no mid-stream retry); §6 Phase 3 work item 3.b; §6 Phase 8 IT-11 |
| T2-E AbortSignal mid-payment double-spend risk | Security H1 | §4.3 "AbortSignal propagation"; §6 Phase 3 work item 3.h/3.i (signal forwarded + warn on abort-mid-retry); §6 Phase 11 scenario 5; §8 Sec-17; IT-10; Unit-6 cancellation; §11 R11 |
| T2-F Caller-supplied PAYMENT-SIGNATURE → double-sign | Security H1 | §4.3 "Fetch-wrapper invariants"; §6 Phase 3 work item 3.d (refuse re-sign); §6 Phase 8 IT-12; §8 Sec-12 |
| T2-G 402 body allowlist | Security H3 | §4.3 "402 body allowlist"; §6 Phase 3 work item 1 (allowlist in parse-402); Unit-2; §8 Sec-14 |
| T2-H `SOLVELA_ALLOW_INSECURE_BASE_URL` production guards | Security H2 | §4.3 "SOLVELA_ALLOW_INSECURE_BASE_URL production guard" (refuses in prod / Vercel Edge / Edge runtime); Unit-1 matrix; §8 Sec-11 |
| T2-I Supply-chain pinning too loose | Security H4 | §5.1 exact-pin `@ai-sdk/openai-compatible`; no install scripts clause; `npm audit` CI job; `npm ls --all` drift guard; §8 Sec-19; §11 R12 |
| T2-J npm publish hygiene | Security H5, Critic P1-5 | §2 new requirements (2FA, OIDC, GPG); §6 Phase 10 work items 4-5 (signed tag, CI-only publish, `--provenance`, tarball SHA verification); §8 Sec-20; §9 release checklist; §11 R13 |
| T2-K Phase 1 + Phase 11 open questions inside phase bodies | Architect P2-7, Critic P1-11 | Phase 1 workspace: resolved — no root `package.json`, independent project (§5.7); Phase 11 ordering: resolved BEFORE Phase 10 (§6 Phase 11 + §12 DAG) |
| T2-L `@solana/web3.js` 2.x / `@solana/kit` transition | Architect P1-5 | §3.9 NEW Open Decision; §10 item 11 follow-up tracker |
| T2-M Signer precedence silent | Security, Architect P1-3 | §3.4 hybrid design clause; §4.1 `SolvelaProviderSettings.wallet` docstring; §6 Phase 2 work item 4 (warn-once on conflict); §6 Phase 4 work item 2; Unit-4; §8 Sec-18; `util/warn-once.ts` |
| T3 supportedUrls multimodal | Architect P2 | §1 Out of Scope (moved multimodal out of v1); §4.3 `supportedUrls` returns `{}`; no IT-12/IT-13 URL tests needed |
| T3 supportsStructuredOutputs default | Critic, Architect P2-1 | §4.1 default `false`; §6 Phase 8 IT-5 covers both opt-in and default paths |
| T3 Model registry codegen path | Architect P2-2 | §3.5 `SOLVELA_MODELS_TOML` env var with default; §6 Phase 5 work item 1 |
| T3 Legacy `RCR_` prefix | Architect P2-3 | §3.10 NEW Open Decision — Solvela-native only; §5.2 env vars table |
| T3 Completion/Embedding deferral via `UnsupportedFunctionalityError` | Architect P2-4 | §4.1 `textEmbeddingModel` + `imageModel` signatures; `@ai-sdk/provider` import |
| T3 Exactly-2-HTTP-calls audit | Security M2 | §6 Phase 3 work item 3.m (counter); §8 Sec-21; Unit-3 assertion; IT-1, IT-2 |
| T3 `SolvelaSigningError` sanitization of upstream message | Security M3 | §6 Phase 6 work item 2 (constructor scrubs base58 + hex); Unit-5; §8 Sec-15 |
| T3 README/MDX key-leak validator | Security M6 | §6 Phase 9 work item 5 (`scripts/check-docs-for-leaked-keys.ts`); §8 Sec-23 |
| T3 Test-mode env safety gating | Security M5 | §4.3 test-mode clause (NODE_ENV === 'test' AND localhost); Unit-1; §8 Sec-22 |
| T3 0.x → 1.0 versioning policy | Critic P1-6 | §9 "Versioning policy" subsection |
| T3 `oh-my-claudecode:test-engineer` agent verification | Critic | §5.4 test-engineer note; §10 process rule 2 fallback clause |
| T3 Hallway test | Critic P1-12 | §1 S13 success criterion; §6 Phase 9 work item 6 |
| T3 CI matrix Node 18 + 20 | Critic | §6 Phase 1 work item 7 (`.github/workflows/ai-sdk-provider.yml`) |
| T3 Bundle size guard | Critic | §5.1 `size-limit` devDep; §6 Phase 1 work item 6; §6 Phase 10 work item 5 |
| T3 Rollback procedure | Critic | §6 Phase 10 work item 9 (`npm deprecate`); §9 |
| T3 Auto-accept conflict with docs | Critic P1-14 | §5.5 "Auto-accept posture" — OFF for Phases 2, 3, 4, 6, 7, 8, 9, 10 |
| NEW §13 Template extraction | Architect | §13 added; three categories (generalizable / AI-SDK-specific / hybrid); recommendation to extract into `docs/plugin-template.md` before authoring next plugin plan |

**Preserved from prior revision (do not change — reviewers commended):**
- §3 Open Decisions option tables with "impact if changed later."
- Test-author separation in per-phase agent assignment columns.
- §8 Security checklist scaffold (Sec-1 TLS, Sec-2 redaction sentinel, Sec-4 retry-bomb, Sec-5 debit-on-success) — expanded, not replaced.
- Callback-first signer design (§3.4 Option C) with `signPayment` as primary path.
- Per-phase agent model specification (sonnet default; opus for Phase 3/4).
- Research citations throughout.
- §12 parallelism DAG (updated order; same DAG philosophy).
- Phase 8 IT-2 (402 twice → payment rejected, exactly 2 calls).
- `@solana/web3.js` as optional peer.

---

## Review Round 2 — P1s Addressed

Round-2 critic confirmed the plan is substantively solid; this surgical pass resolves seven polish items without restructuring phases, agents, or §3 Open Decisions.

| # | Finding | Where the fix landed |
|---|---|---|
| P1-1 | Codegen path off-by-one (§3.5 vs Phase 5 WI-1 mismatch) | §3.5 "Codegen path (T3 polish)" — corrected to `path.resolve(__dirname, '../../../config/models.toml')` (three `..`); matches Phase 5 WI-1 unchanged |
| P1-2 | Fetch-wrapper seam for `APICallError` catch is architecturally wrong | §4.3 "Error surface sanitization" — rewritten to document **Option A seam** (wrapper inspects non-2xx before return; throws `SolvelaUpstreamError` directly); §6 Phase 3 WI-3.k — rewritten to match; §4.1 exports `SolvelaUpstreamError`; §6 Phase 6 WI-2 adds `SolvelaUpstreamError` class; §8 Sec-3 — clarified seam; §6 Phase 8 IT-13 — rewritten to assert `SolvelaUpstreamError` type + sanitized fields |
| P1-3 | `SolvelaInvalidConfigError` didn't run `sanitizeError` (inconsistency) | §6 Phase 6 WI-2 item 4 — constructor now runs `sanitizeError` on `message` + context (consistency with other three error classes) |
| P1-4 | Scheme-selection rule missing in budget spec | §4.3 new "Scheme selection from `accepts[]`" block (v1 rule: first `scheme: 'exact'` + `asset: USDC`; future v2 note); §4.3 budget state machine — `cost` derivation cites the rule; §6 Phase 3 WI-1 — `parse-402.ts` exports `selectAccept` with the v1 rule; §7 Phase 7 Unit-2 — scheme-selection test added (multi-entry pick, zero-match throws) |
| P1-5 | No fallback for `oh-my-claudecode:security-reviewer` agent | §5.4 new paragraph — security-reviewer availability verification + fallback to `oh-my-claudecode:code-reviewer` with explicit security-only prompt covering applicable Sec-1..Sec-23 items; §10 rule 2 — parallel fallback clause added under the "Security" bullet |
| P1-6 | `examples/` exclusion from `npm pack` undecided | Decision A (ship `examples/` with package): §5.1 new "`package.json` `files` array" block — explicit `files: ["dist/", "examples/", "README.md", "LICENSE"]`; §6 Phase 9 WI-2 — notes `examples/` ships with package; §6 Phase 10 WI-1 — tarball contents updated to include `examples/`; additional `examples/` leak sub-check added |
| P1-7 | Phase 2 WI-4 done criterion referenced `generateText` but `.refine()` throws at construction | Phase 2 "Done criteria" — rewritten to Option A: asserts `createSolvelaProvider({})` throws `SolvelaInvalidConfigError` at construction time (simpler, matches the WI-3 `.refine()` design). `generateText` is not invoked. |

**No items handled differently than instructed.** Option A was chosen for both P1-2 (recommended) and P1-7 (recommended, matches `.refine()`); Decision A chosen for P1-6 (recommended).

---

## Appendix B — Review Round 3: Ecosystem Research Findings Applied

Three independent researchers surveyed the ecosystem for better answers to the plan's 10 Open Decisions. The user approved the research-informed set. Round 3 is a targeted in-place revision: no phase reordering, no agent-assignment changes outside Phase 4 (which had its focus shift from "signer abstraction" to "reference adapter implementation"). All Round 1 + Round 2 findings remain addressed; this appendix layers on top of Appendix A.

**Note on Appendix A overlap**: Findings T1-E (`OpaquePrivateKey` branded type) and T1-F (wallet runtime-gating) were originally addressed by the `OpaquePrivateKey` + `assertNodeRuntime` mitigations described in Appendix A. Those mitigations are **superseded** by Round 3 Change 2 (adapter interface pattern). The adapter pattern subsumes both concerns: adapters are opaque to the provider (T1-E), and browser/Edge safety becomes each adapter's responsibility via sub-export tree-shaking rather than runtime refusal (T1-F).

| # | Round 3 change | Sections updated |
|---|---|---|
| 1 | **Interface version V4 → V3.** Target `LanguageModelV3` + stable `ai@^6.0.0`; document V3→V4 upgrade gate. | §2 (recommendation flip + upgrade-path bullet); §3.1 (rewritten — Option A reversal, ecosystem rationale, upgrade path); §4.1 (TypeScript imports + `LanguageModelV3` returns + `specificationVersion: 'v3'`); §4.3 (mapping table — no V4 references in our shipping path); §5.1 (peer-deps switched to stable channel; exact-pin `@ai-sdk/openai-compatible` 2.x); §6 Phase 2 WI-3+WI-6 (zod schema + tests assert `'v3'`); §7 Phase 7 Unit-7 (assert `specificationVersion: 'v3'`); §9 Versioning policy (V3→V4 upgrade gate as 0.2 trigger); §11 R1 (risk lower — stable channel). |
| 2 | **Signer design → adapter interface.** Drop `OpaquePrivateKey` hybrid; replace with `SolvelaWalletAdapter` interface; reference `createLocalWalletAdapter` ships from `./adapters/local` sub-export. | §2 (recommendation flip); §3.4 (entirely rewritten — Option B reversal, four-ecosystem rationale, public interface + settings + exports map + what-stays/what-removed); §4.1 (settings shape — `wallet: SolvelaWalletAdapter` required; `apiKey` clarified as scope token; reference adapter sub-export); §4.2 (`wallet-adapter.ts` + `adapters/local.ts` replace `wallet-key.ts`+`runtime-gate.ts`+`signer.ts`); §4.3 mapping table (signer adapter row rewritten); §5.1 `exports` map block updated with `./adapters/local`; §5.2 (`SOLANA_WALLET_KEY` becomes adapter-implementation concern, not first-class env var); §6 Phase 2 (WI-1..WI-6 simplified, deliverables list updated, done criteria adjusted to no-wallet rejection); §6 Phase 4 (renamed "Reference adapter implementation"; WI-1 = `createLocalWalletAdapter` + Kit-conditional; WI-2 = direct adapter invocation; WI-3 = adapter-contract tests; deliverables drop `signer.ts`); §6 Phase 6 (errors stay; OpaquePrivateKey paths gone — preserved in spirit via constructor sanitization); §7 Phase 7 Unit-9 rewritten to "adapter-contract" test; §8 Sec-13 rewritten as adapter-contract security check; §8 Sec-18 deleted; §11 R9 risk model revised (was wallet runtime exposure → now adapter-implementation quality); §13 generalizable-template bullet updated for cross-framework reuse. |
| 3 | **Model coverage `(string & {})` escape hatch.** Codegen union retains 26 known + `(string & {})` tail. | §3.5 (option table + recommendation refined to include `(string & {})` rationale); §6 Phase 5 WI-1 (codegen template emits `\| (string & {})`); §7 Phase 7 Unit-8 (test asserts hypothetical-future-model produces no TS error). |
| 4 | **Docs ownership inverted.** README = minimal quickstart + link; MDX = canonical reference. | §3.7 (rewritten with Option C reversal + Stripe/Supabase/Vercel-community precedent); §6 Phase 9 WI-1 (README scoped to install + 10-line quickstart + link); §6 Phase 9 WI-3 (MDX expanded with full feature set + adapter authoring guide as canonical); §6 Phase 9 Done criteria (Round 3 ship gate adds MDX commit + Fumadocs render). |
| 5 | **Spike pass-bar = Turbopack `next build`, not just Node.** Three scenarios; pass-bar is BOTH (a) bare Node 20 ESM AND (b) Next.js 16 Turbopack. | §3.8 (spike paragraph rewritten with three scenarios + Turbopack-specific failure likelihood per #78267); §6 Phase 1 WI-9 (three numbered sub-scenarios, blocker/informational marked, §3.9 Round 3 cross-link to Kit-first if Option B triggers); §11 R6 (Turbopack-specific risk escalated to High; mitigation cites pass-bar). |
| 6 | **Kit-first conditional for `@solvela/signer-core`.** If §3.8 Option B triggers, the new package MUST be built on `@solana/kit`, not web3.js 1.x. | §3.9 (conditional clause added with `@solana/kit` + `@solana-program/token` + `solana-dev` skill rationale); §6 Phase 4 WI-1 (Kit-conditional implementation block); §11 R10 (rebuilt — web3.js-based signer-core risk escalates from Medium to High when triggered). |
| 7 | **§3.3 — revisit at v1.0.** In-repo OK for v0.1 launch; industry precedent favors separate repo by v1.0; review at v1.0 milestone. | §3.3 (rationale + revisit caveat with Stripe/Supabase/Vercel-community precedent + `git subtree split` history-preservation note); §9 Versioning policy (v1.0 milestone explicitly includes repo-split decision review). |

**Items preserved unchanged from Round 2 (do not modify):**
- §3 Open Decisions option-table format with "impact if changed later."
- Phase ordering DAG (T2-A revised order: Phase 1 → Phase 6 → Phase 2 → Phase 3 → Phase 4; Phase 11 before Phase 10).
- Per-phase agent assignment for Phases 1, 2, 3, 5, 6, 7, 8, 9, 10, 11 (Phase 4 work-item focus shifts but agents/models unchanged: executor opus, security-reviewer, silent-failure-hunter, test-engineer).
- Test-author separation (saved memory `feedback_test_author_separation.md`).
- §8 Security checklist scaffold (Sec-1 through Sec-23) — Sec-13 rewritten + Sec-18 deleted; all others preserved.
- §4.3 Error surface sanitization (Option A seam) and budget reservation/debit state machine.
- §4.3 Scheme selection from `accepts[]` (P1-4 rule).
- All Round 1 fixes (T1-A through T2-M) and all Round 2 fixes (P1-1 through P1-7).
- Appendix A intact in full.

**No items handled differently than instructed.** All seven changes applied as specified. The V3 → V4 migration path is documented in §3.1 ("Upgrade path"), §9 ("V3 → V4 upgrade gate"), and §11 R1; `LanguageModelV4` citations to the research file are preserved as forward-looking analysis (the research's V3-vs-V4 comparison remains the authoritative source for the future upgrade) — the plan's shipping target is V3.
