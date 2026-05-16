<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-05-10 -->

# sdks

## Purpose
Client SDKs so agents in different languages/runtimes can talk to the Solvela gateway. Each SDK: loads a local wallet, signs SPL-USDC transactions client-side, constructs x402 headers, retries on 402, and surfaces OpenAI-compatible chat responses.

## Key Files
_(no loose files — see subdirectories)_

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `go/` | Redirect stub — canonical Go SDK lives at [solvela-ai/solvela-go](https://github.com/solvela-ai/solvela-go) |
| `typescript/` | TypeScript/Node SDK (see `typescript/AGENTS.md`) |
| `python/` | Python SDK (see `python/AGENTS.md`) |
| `mcp/` | MCP server exposing Solvela as tools to MCP clients (see `mcp/AGENTS.md`) |

## For AI Agents

### Working In This Directory
- **Private keys never leave the client.** SDKs sign locally; the gateway only sees signed transactions.
- Each SDK stays independent — no shared codegen today, but API surfaces are intentionally parallel so features port easily.
- x402 contract: send request → receive 402 with `payment_required` body → sign USDC-SPL transfer → retry with `PAYMENT-SIGNATURE: <base64-or-json>` header.
- 5% platform fee is already baked into the 402 response's `cost_breakdown` — SDKs surface it, don't recompute it.

#### Known npm audit advisories — `bigint-buffer` ecosystem block

All three TypeScript SDKs (`mcp`, `openclaw-provider`, `ai-sdk-provider`) report the same 3-advisory chain on `npm audit`, all rooted in a single CVE:

| pkg | severity | source |
|---|---|---|
| `bigint-buffer` | high | [GHSA-3gc7-fjrx-p6mg](https://github.com/advisories/GHSA-3gc7-fjrx-p6mg) — buffer overflow in `toBigIntLE()` |
| `@solana/buffer-layout-utils` | high | transitive via `bigint-buffer` |
| `@solana/spl-token` | high | transitive via `@solana/buffer-layout-utils` |

**No upstream fix exists.** `bigint-buffer`'s maintainer hasn't patched the vulnerability. The npm-suggested "fix" (`@solana/spl-token` 0.1.8) is a semver-major regression that drops modern SPL features the SDKs depend on. This is a Solana-ecosystem-wide block — every project that depends on `@solana/web3.js` v1 + `@solana/spl-token` for SPL operations inherits the same chain.

**Reachability analysis:**

`toBigIntLE()` is called by `@solana/buffer-layout-utils` during SPL account-state decode and instruction-data encode. In our SDKs:

- **Outbound transfers** (the hot path): instruction buffers are constructed by `@solana/spl-token`'s typed builders from values we own (amount, mint, recipient pubkey). No untrusted bytes flow through `toBigIntLE()` here.
- **Account-state decode**: none of the three SDKs decode arbitrary remote SPL accounts on the hot path. mcp's deposit flow constructs and broadcasts; signer-core builds the deposit tx; openclaw-provider/ai-sdk-provider wrap signer-core. The CVE's exploit prerequisite (attacker-controlled buffer feeding `toBigIntLE()`) is therefore not reachable through the SDK surfaces we expose.

The vulnerability is reportable but not exploitable in our usage. Accept as an ecosystem-wide tolerated risk.

**CI compromise:** the three SDK workflows (`.github/workflows/{mcp,openclaw-provider,ai-sdk-provider}.yml`) all run `npm audit --production --audit-level=critical` rather than `--audit-level=high`. This catches CRITICAL regressions while tolerating the known 3-advisory High chain. Inline comments in each workflow point at this section.

**Re-evaluate when** ANY of:

- `bigint-buffer` ships a patched release (track via the GHSA page or `npm view bigint-buffer versions`).
- The SDK migrates to `@solana/web3.js` v2 (different transitive graph; CVE may not apply).
- A peer package outside the three TS SDKs (e.g. dashboard, cli-npm shim, python) starts depending on `@solana/spl-token` or `bigint-buffer` directly (currently they don't — verified via `grep '"@solana/spl-token"' dashboard/package.json sdks/cli-npm/package.json`).

If a re-evaluation determines the advisory is exploitable in a new use case, flip the CI from `--audit-level=critical` back to `--audit-level=high` *in the same PR* that addresses the actual vulnerability.

Related: PR #134 (surfaced the warning), issue #136 (this triage).

### Testing Requirements
Each SDK has its own test runner — see its `AGENTS.md`.

### Common Patterns
- Minimal public surface: `Client`, `Wallet`, `chat()`.
- Errors typed per-language (Go `errors.Is`, Python custom exceptions, TS discriminated unions).

## Dependencies

### Internal
- Solvela gateway HTTP contract; shared wire types live in `../crates/protocol/` (SDKs reimplement these per-language).

### External
- Per-language HTTP and crypto libraries (see each SDK's AGENTS.md).

<!-- MANUAL: -->
