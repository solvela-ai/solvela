# Solvela Rebrand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rebrand Solvela to Solvela across the entire repository -- crate names, binary names, env vars, HTTP headers, SDK packages, infrastructure configs, and documentation.

**Architecture:** Mechanical rename organized by blast radius. Each task produces a compilable, test-passing state. Env vars and HTTP headers use dual-accept/dual-emit for backward compatibility during the transition period. The `x402` crate keeps its name (protocol, not brand). Escrow program ID and PDA seeds are unchanged.

**Tech Stack:** Rust (Cargo workspace), Python (hatch), TypeScript (npm), Go modules, Docker, Fly.io, Next.js dashboard

**Brand architecture (non-negotiable):**
- **Solvela.ai** -- the payment gateway platform (THIS repo)
- **RustyClaw.ai** -- trading terminal (SEPARATE repo, NOT part of this rename)
- **Telsi.ai** -- AI assistant SaaS (separate, keeps its name)

---

## File Structure Overview

Files modified per task are listed in each task section. Here is the summary of all files touched:

**Task 1 (Cargo crate renames):** 10 Cargo.toml files + ~25 Rust source files with `use rustyclaw_protocol` imports + Cargo.lock
**Task 2 (CLI binary rename):** 2 files (cli Cargo.toml, gateway main.rs env comment)
**Task 3 (Env var dual-accept):** ~5 source files + .env.example + config/default.toml + dashboard settings
**Task 4 (HTTP header dual-emit):** 3 source files + 1 integration test file + CI smoke tests
**Task 5 (SDK package renames):** ~15 files across sdks/python, sdks/typescript, sdks/go, sdks/mcp
**Task 6 (Infrastructure configs):** fly.toml, docker-compose.yml, Dockerfile, .github/workflows/ci.yml
**Task 7 (Documentation):** CLAUDE.md, HANDOFF.md, README.md, CHANGELOG.md, config/default.toml comments, code comments
**Task 8 (Prometheus metrics rename):** 1 source file + CI smoke test assertion
**Task 9 (API key prefix):** 3 source files (orgs/queries.rs, orgs/models.rs, middleware/api_key.rs)
**Task 10 (Final verification):** No file changes, just test runs

---

### Task 1: Cargo Workspace Crate Renames

This is the highest blast-radius change. Rename crate packages and update all `use` statements.

**Rename mapping:**
| Old name | New name | Rust import |
|---|---|---|
| `rustyclaw-protocol` | `solvela-protocol` | `use solvela_protocol::` |
| `gateway` (package) | `gateway` (unchanged) | -- |
| `rustyclawrouter` (binary) | `solvela-gateway` (binary) | -- |
| `rustyclawrouter-cli` (package) | `solvela-cli` (package) | -- |
| `rcr` (binary) | `solvela` (binary) | -- |
| `rustyclawrouter-escrow` (package) | `solvela-escrow` (package) | -- |
| `rustyclawrouter_escrow` (lib name) | `solvela_escrow` (lib name) | -- |
| `router` (package) | `solvela-router` (package) | -- |
| `x402` | `x402` (unchanged) | -- |

**Files:**
- Modify: `Cargo.toml` (workspace root)
- Modify: `crates/protocol/Cargo.toml`
- Modify: `crates/router/Cargo.toml`
- Modify: `crates/gateway/Cargo.toml`
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/x402/Cargo.toml`
- Modify: `programs/escrow/Cargo.toml`
- Modify: All files containing `use rustyclaw_protocol::` (see list below)
- Modify: `crates/x402/src/types.rs` (`pub use rustyclaw_protocol::*`)
- Regenerate: `Cargo.lock`

**Source files with `use rustyclaw_protocol::`** (from grep, excluding worktrees):
- `crates/gateway/tests/integration.rs`
- `crates/gateway/src/a2a/handler.rs`
- `crates/gateway/src/providers/mod.rs`
- `crates/gateway/src/providers/xai.rs`
- `crates/gateway/src/providers/heartbeat.rs`
- `crates/gateway/src/providers/fallback.rs`
- `crates/gateway/src/providers/openai.rs`
- `crates/gateway/src/providers/google.rs`
- `crates/gateway/src/providers/deepseek.rs`
- `crates/gateway/src/providers/anthropic.rs`
- `crates/gateway/src/cache.rs`
- `crates/gateway/src/middleware/prompt_guard.rs`
- `crates/router/src/models.rs`
- `crates/router/src/scorer.rs`
- `crates/protocol/src/lib.rs` (doc comment only)
- `crates/gateway/src/routes/pricing.rs`
- `crates/x402/src/types.rs`
- `crates/gateway/src/routes/chat/mod.rs`
- `crates/gateway/src/routes/chat/provider.rs`
- `crates/gateway/src/routes/chat/cost.rs`

- [ ] **Step 1: Rename `rustyclaw-protocol` crate**

In `crates/protocol/Cargo.toml`, change:
```toml
[package]
name = "solvela-protocol"
version = "0.1.0"
edition = "2021"
description = "Shared wire-format types for the Solvela ecosystem (x402 payment protocol + OpenAI-compatible chat types)"
license = "MIT OR Apache-2.0"
repository = "https://github.com/sky64/Solvela"
```

- [ ] **Step 2: Rename `router` crate**

In `crates/router/Cargo.toml`, change:
```toml
[package]
name = "solvela-router"
```

And update its dependency:
```toml
[dependencies]
solvela-protocol = { workspace = true }
```

- [ ] **Step 3: Rename gateway binary**

In `crates/gateway/Cargo.toml`, change:
```toml
[[bin]]
name = "solvela-gateway"
path = "src/main.rs"

[dependencies]
solvela-protocol = { workspace = true }
solvela-router = { workspace = true }
```

(The package name `gateway` stays the same -- only the binary name changes.)

- [ ] **Step 4: Rename CLI crate and binary**

In `crates/cli/Cargo.toml`, change:
```toml
[package]
name = "solvela-cli"

[[bin]]
name = "solvela"
path = "src/main.rs"

[dependencies]
solvela-router = { workspace = true }
```

- [ ] **Step 5: Rename escrow crate**

In `programs/escrow/Cargo.toml`, change:
```toml
[package]
name = "solvela-escrow"
description = "Anchor escrow program for Solvela USDC-SPL payments"

[lib]
name = "solvela_escrow"
```

Also update the comment at the top of the file to reference Solvela instead of Solvela.

In `programs/escrow/src/lib.rs`, update:
- Doc comment: `//! Solvela Escrow Program` -> `//! Solvela Escrow Program`
- Module name: `pub mod rustyclawrouter_escrow {` -> `pub mod solvela_escrow {`

In `programs/escrow/Anchor.toml`, update all program key names:
```toml
[programs.localnet]
solvela_escrow = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"

[programs.devnet]
solvela_escrow = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"

[programs.mainnet]
solvela_escrow = "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU"
```

**Note:** The program ID (`9neDH...`) stays the same -- it's derived from `declare_id!`, not the module name. The module name only affects the IDL name.

In `programs/escrow/tests/unit.rs`, replace all `rustyclawrouter_escrow::` with `solvela_escrow::`:
- `use rustyclawrouter_escrow::state::Escrow` -> `use solvela_escrow::state::Escrow`
- `rustyclawrouter_escrow::ID` -> `solvela_escrow::ID`

In `programs/escrow/tests/helpers.rs`, update the `.so` path:
```rust
const PROGRAM_SO: &[u8] = include_bytes!("../target/deploy/solvela_escrow.so");
```

**Note:** After renaming the lib, `anchor build` will produce `target/deploy/solvela_escrow.so` instead of `rustyclawrouter_escrow.so`. Existing CI that references the old `.so` path must be updated.

- [ ] **Step 6: Update workspace root Cargo.toml**

In `Cargo.toml` (root), change:
```toml
[workspace.package]
repository = "https://github.com/sky64/Solvela"

[workspace.dependencies]
solvela-router = { path = "crates/router" }
solvela-protocol = { path = "crates/protocol" }
```

Remove the old `rustyclaw-protocol` and `router` entries. Keep `x402` unchanged.

- [ ] **Step 7: Update x402 crate dependency**

In `crates/x402/Cargo.toml`, change:
```toml
[dependencies]
solvela-protocol = { workspace = true }
```

- [ ] **Step 8: Rename all `use rustyclaw_protocol::` to `use solvela_protocol::`**

In every file listed above, perform a global search-and-replace:
- `use rustyclaw_protocol::` -> `use solvela_protocol::`
- `rustyclaw_protocol::{` -> `solvela_protocol::{` (same pattern, just ensuring multi-line imports are caught)
- `pub use rustyclaw_protocol::*` -> `pub use solvela_protocol::*` (in `crates/x402/src/types.rs`)

Also update the doc comment in `crates/protocol/src/lib.rs`:
```rust
//   use solvela_protocol::{ChatRequest, PaymentRequired, CostBreakdown};
```

- [ ] **Step 9: Rename all `use router::` to `use solvela_router::`**

The `router` crate is renamed to `solvela-router`, so `use router::` becomes `use solvela_router::`. Update these 15 files:

- `crates/gateway/tests/integration.rs` (`use router::models::ModelRegistry`)
- `crates/gateway/src/a2a/jsonrpc.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/a2a/handler.rs` (`use router::profiles`, `use router::scorer`, `use router::models::ModelRegistry` in test)
- `crates/gateway/src/a2a/agent_card.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/main.rs` (`use router::models::ModelRegistry`)
- `crates/gateway/src/lib.rs` (`use router::models::ModelRegistry`)
- `crates/gateway/src/middleware/api_key.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/routes/health.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/routes/orgs/mod.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/routes/orgs/teams.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/routes/orgs/api_keys.rs` (`use router::models::ModelRegistry` in test)
- `crates/gateway/src/routes/chat/mod.rs` (`use router::profiles`, `use router::scorer`)
- `crates/gateway/src/routes/chat/provider.rs` (check for `use router::`)

Global replace: `use router::` -> `use solvela_router::`

The workspace dependency references in `crates/gateway/Cargo.toml` and `crates/cli/Cargo.toml` were already updated in Steps 3 and 4 (changing `router = { workspace = true }` to `solvela-router = { workspace = true }`).

- [ ] **Step 10: Regenerate Cargo.lock**

Run: `cargo generate-lockfile`

- [ ] **Step 11: Verify compilation**

Run: `cargo check --all-targets`
Expected: Clean compilation, zero errors.

- [ ] **Step 12: Run all tests**

Run: `cargo test --all`
Expected: All tests pass.

- [ ] **Step 13: Run clippy**

Run: `cargo clippy --all-targets --all-features -- -D warnings`
Expected: Zero warnings.

- [ ] **Step 14: Commit**

```bash
git add Cargo.toml Cargo.lock crates/protocol/Cargo.toml crates/router/Cargo.toml \
  crates/gateway/Cargo.toml crates/cli/Cargo.toml crates/x402/Cargo.toml \
  programs/escrow/Cargo.toml crates/ 
git commit -m "refactor: rename Cargo crates from rustyclaw to solvela

Rename rustyclaw-protocol -> solvela-protocol, router -> solvela-router,
rustyclawrouter (binary) -> solvela-gateway, rustyclawrouter-cli -> solvela-cli,
rustyclawrouter-escrow -> solvela-escrow. x402 crate unchanged (protocol name).
All use statements updated to solvela_protocol / solvela_router."
```

---

### Task 2: CLI Binary Rename (rcr -> solvela)

The binary rename was done in Task 1 (Cargo.toml). This task handles any remaining references to the `rcr` binary name in source code and scripts.

**Files:**
- Modify: `crates/cli/src/main.rs` (clap app name/about if present)
- Modify: `scripts/load-test.sh` (if it references `rcr` binary)
- Check: Any other scripts referencing `rcr` binary

- [ ] **Step 1: Update CLI app metadata**

In `crates/cli/src/main.rs`, find the clap derive or builder that sets the app name. Update references from "rcr" to "solvela" and from "Solvela" to "Solvela" in help text/descriptions.

- [ ] **Step 2: Search for `rcr` binary references in scripts**

Run: `grep -rn '"rcr"' scripts/ && grep -rn './rcr' scripts/ && grep -rn 'cargo run -p rustyclawrouter-cli' .`

Update any matches to reference `solvela` binary / `solvela-cli` package.

- [ ] **Step 3: Verify CLI builds and runs**

Run: `cargo build -p solvela-cli && ./target/debug/solvela --help`
Expected: Help output shows "solvela" branding.

- [ ] **Step 4: Run CLI tests**

Run: `cargo test -p solvela-cli`
Expected: All tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/cli/ scripts/
git commit -m "refactor: rename CLI binary from rcr to solvela"
```

---

### Task 3: Env Var Dual-Accept (SOLVELA_ + RCR_ fallback)

Add dual-accept: check `SOLVELA_*` first, fall back to `RCR_*` with a deprecation warning. This is a backward-compatible change.

**Files:**
- Modify: `crates/gateway/src/main.rs`
- Modify: `crates/gateway/src/lib.rs`
- Modify: `.env.example`
- Modify: `config/default.toml`
- Modify: `dashboard/src/app/settings/page.tsx`
- Modify: `dashboard/src/app/wallet/page.tsx`
- Test: `crates/gateway/tests/integration.rs` (add env var fallback test if feasible)

**Env var mapping (30 vars):**
| Old prefix | New prefix | Example |
|---|---|---|
| `RCR_ENV` | `SOLVELA_ENV` | |
| `RCR_DEV_BYPASS_PAYMENT` | `SOLVELA_DEV_BYPASS_PAYMENT` | |
| `RCR_SERVER_HOST` | `SOLVELA_SERVER_HOST` | |
| `RCR_SERVER_PORT` | `SOLVELA_SERVER_PORT` | |
| `RCR_SOLANA_RPC_URL` | `SOLVELA_SOLANA_RPC_URL` | |
| `RCR_SOLANA_RECIPIENT_WALLET` | `SOLVELA_SOLANA_RECIPIENT_WALLET` | |
| `RCR_SOLANA_FEE_PAYER_KEY` | `SOLVELA_SOLANA_FEE_PAYER_KEY` | |
| `RCR_SOLANA_ESCROW_PROGRAM_ID` | `SOLVELA_SOLANA_ESCROW_PROGRAM_ID` | |
| `RCR_SOLANA__RPC_URL` | `SOLVELA_SOLANA__RPC_URL` | Fly.io double-underscore |
| `RCR_SOLANA__RECIPIENT_WALLET` | `SOLVELA_SOLANA__RECIPIENT_WALLET` | |
| `RCR_SOLANA__USDC_MINT` | `SOLVELA_SOLANA__USDC_MINT` | |
| `RCR_SOLANA__ESCROW_PROGRAM_ID` | `SOLVELA_SOLANA__ESCROW_PROGRAM_ID` | |
| `RCR_SOLANA__FEE_PAYER_KEY` | `SOLVELA_SOLANA__FEE_PAYER_KEY` | |
| `RCR_SOLANA__FEE_PAYER_KEY_N` | `SOLVELA_SOLANA__FEE_PAYER_KEY_N` | |
| `RCR_SOLANA__NONCE_ACCOUNT` | `SOLVELA_SOLANA__NONCE_ACCOUNT` | |
| `RCR_HOST` | `SOLVELA_HOST` | |
| `RCR_PORT` | `SOLVELA_PORT` | |
| `RCR_LOG_FORMAT` | `SOLVELA_LOG_FORMAT` | |
| `RCR_CORS_ORIGINS` | `SOLVELA_CORS_ORIGINS` | |
| `RCR_ADMIN_TOKEN` | `SOLVELA_ADMIN_TOKEN` | |
| `RCR_SESSION_SECRET` | `SOLVELA_SESSION_SECRET` | |
| `RCR_DB_MAX_CONNECTIONS` | `SOLVELA_DB_MAX_CONNECTIONS` | |
| `RCR_REQUEST_TIMEOUT_SECS` | `SOLVELA_REQUEST_TIMEOUT_SECS` | |
| `RCR_MAX_CONCURRENT_REQUESTS` | `SOLVELA_MAX_CONCURRENT_REQUESTS` | |
| `RCR_DEV_BYPASS_PAYMENT` | `SOLVELA_DEV_BYPASS_PAYMENT` | |
| `RCR_DAILY_BUDGET_USDC` | `SOLVELA_DAILY_BUDGET_USDC` | |
| `RCR_MONTHLY_BUDGET_USDC` | `SOLVELA_MONTHLY_BUDGET_USDC` | |
| `RCR_PROMPT_GUARD_ENABLED` | `SOLVELA_PROMPT_GUARD_ENABLED` | |
| `RCR_RATE_LIMIT_ENABLED` | `SOLVELA_RATE_LIMIT_ENABLED` | |

- [ ] **Step 1: Create a helper function for dual-accept env var reads**

Add a utility function in `crates/gateway/src/main.rs` (or a new `crates/gateway/src/env_compat.rs` module):

```rust
/// Read an environment variable with dual-accept: try SOLVELA_ prefix first,
/// fall back to RCR_ prefix with a deprecation warning.
fn env_dual(solvela_name: &str, rcr_name: &str) -> Result<String, std::env::VarError> {
    match std::env::var(solvela_name) {
        Ok(val) => Ok(val),
        Err(_) => match std::env::var(rcr_name) {
            Ok(val) => {
                tracing::warn!(
                    old = rcr_name,
                    new = solvela_name,
                    "deprecated env var used -- please migrate to {solvela_name}"
                );
                Ok(val)
            }
            Err(e) => Err(e),
        },
    }
}
```

- [ ] **Step 2: Replace all `std::env::var("RCR_...")` calls in main.rs**

Replace each `std::env::var("RCR_XYZ")` with `env_dual("SOLVELA_XYZ", "RCR_XYZ")`. For the double-underscore variants (Fly.io), do the same:

Example transformation:
```rust
// Before:
std::env::var("RCR_SOLANA__RPC_URL").or_else(|_| std::env::var("RCR_SOLANA_RPC_URL"))

// After:
env_dual("SOLVELA_SOLANA__RPC_URL", "RCR_SOLANA__RPC_URL")
    .or_else(|_| env_dual("SOLVELA_SOLANA_RPC_URL", "RCR_SOLANA_RPC_URL"))
```

- [ ] **Step 3: Replace all `std::env::var("RCR_...")` calls in lib.rs**

Same pattern for all env var reads in `crates/gateway/src/lib.rs`.

- [ ] **Step 4: Update `.env.example`**

Change all `RCR_` prefixed vars to `SOLVELA_` as the primary. Add a comment block at the top:

```bash
# Solvela Gateway environment variables
# (Legacy RCR_ prefix is still accepted with a deprecation warning)
```

Replace every `RCR_` with `SOLVELA_` in the file. Update the header comment from "Solvela" to "Solvela Gateway". Update `DATABASE_URL` comment to reference `solvela` instead of `rustyclawrouter`:
```
DATABASE_URL=postgres://solvela:solvela_dev_password@localhost:5432/solvela
```

- [ ] **Step 5: Update `config/default.toml` comments**

Change the header from `# Solvela Gateway Configuration` to `# Solvela Gateway Configuration`. Update the comment `# Set via RCR_SOLANA_RECIPIENT_WALLET env var` to `# Set via SOLVELA_SOLANA_RECIPIENT_WALLET env var`.

- [ ] **Step 6: Update dashboard env var references**

In `dashboard/src/app/settings/page.tsx`, replace all `RCR_` with `SOLVELA_` in the template literals and description strings.

In `dashboard/src/app/wallet/page.tsx`, replace:
```typescript
process.env.RCR_SOLANA_RECIPIENT_WALLET ??
"Configure RCR_SOLANA_RECIPIENT_WALLET in .env"
```
with:
```typescript
process.env.SOLVELA_SOLANA_RECIPIENT_WALLET ??
process.env.RCR_SOLANA_RECIPIENT_WALLET ??
"Configure SOLVELA_SOLANA_RECIPIENT_WALLET in .env"
```

- [ ] **Step 7: Verify compilation and tests**

Run: `cargo check --all-targets && cargo test --all`
Expected: All pass. The tests don't set `RCR_` env vars explicitly (they use test_app()), so they should be unaffected.

- [ ] **Step 8: Commit**

```bash
git add crates/gateway/src/main.rs crates/gateway/src/lib.rs .env.example \
  config/default.toml dashboard/src/
git commit -m "feat: add SOLVELA_ env var prefix with RCR_ fallback and deprecation warning

All RCR_ env vars now have SOLVELA_ equivalents. The gateway checks SOLVELA_
first, falls back to RCR_ with a tracing::warn deprecation notice. .env.example
updated to use new prefix. Dashboard references updated."
```

---

### Task 4: HTTP Header Dual-Emit (x-solvela- + x-rcr-)

Emit both `x-solvela-*` and `x-rcr-*` response headers during the transition period. Accept both `x-solvela-debug` and `x-rcr-debug` as input.

**Files:**
- Modify: `crates/gateway/src/routes/debug_headers.rs`
- Modify: `crates/gateway/src/middleware/request_id.rs`
- Modify: `crates/gateway/src/routes/chat/mod.rs` (session, fallback headers)
- Modify: `crates/gateway/src/routes/chat/provider.rs` (fallback-preference input header)
- Modify: `crates/gateway/src/routes/proxy.rs` (request-id forwarding)
- Modify: `crates/gateway/src/lib.rs` (CORS exposed headers list)
- Modify: `crates/gateway/tests/integration.rs` (update header assertions)
- Modify: `.github/workflows/ci.yml` (smoke test header checks)

**Header mapping:**
| Old | New (primary) | Behavior |
|---|---|---|
| `x-rcr-request-id` | `x-solvela-request-id` | Emit both on response |
| `x-rcr-debug` | `x-solvela-debug` | Accept both on request |
| `x-rcr-model` | `x-solvela-model` | Emit both on response |
| `x-rcr-tier` | `x-solvela-tier` | Emit both on response |
| `x-rcr-score` | `x-solvela-score` | Emit both on response |
| `x-rcr-profile` | `x-solvela-profile` | Emit both on response |
| `x-rcr-provider` | `x-solvela-provider` | Emit both on response |
| `x-rcr-cache` | `x-solvela-cache` | Emit both on response |
| `x-rcr-latency-ms` | `x-solvela-latency-ms` | Emit both on response |
| `x-rcr-payment-status` | `x-solvela-payment-status` | Emit both on response |
| `x-rcr-token-estimate-in` | `x-solvela-token-estimate-in` | Emit both on response |
| `x-rcr-token-estimate-out` | `x-solvela-token-estimate-out` | Emit both on response |
| `x-rcr-session` | `x-solvela-session` | Emit both on response |
| `x-rcr-fallback` | `x-solvela-fallback` | Emit both on response |
| `x-rcr-fallback-preference` | `x-solvela-fallback-preference` | Accept both on request |

- [ ] **Step 1: Update `debug_headers.rs` -- add dual-emit statics**

For each existing `static H_*: HeaderName`, add a corresponding `static H_*_NEW: HeaderName` with the `x-solvela-` prefix. Update `attach_debug_headers` to insert both:

```rust
static H_MODEL: HeaderName = HeaderName::from_static("x-rcr-model");
static H_MODEL_NEW: HeaderName = HeaderName::from_static("x-solvela-model");
// ... repeat for all 10 debug headers

pub static DEBUG_FLAG_HEADER: HeaderName = HeaderName::from_static("x-rcr-debug");
pub static DEBUG_FLAG_HEADER_NEW: HeaderName = HeaderName::from_static("x-solvela-debug");
```

Update `is_debug_enabled` to check both headers:
```rust
pub fn is_debug_enabled(headers: &HeaderMap) -> bool {
    headers
        .get(&DEBUG_FLAG_HEADER_NEW)
        .or_else(|| headers.get(&DEBUG_FLAG_HEADER))
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}
```

Update `attach_debug_headers` to emit both old and new headers.

- [ ] **Step 2: Update `request_id.rs` -- dual-emit request ID**

Add a new static:
```rust
pub static REQUEST_ID_HEADER_NEW: HeaderName = HeaderName::from_static("x-solvela-request-id");
```

In the `call` method, insert both headers:
```rust
response.headers_mut().insert(REQUEST_ID_HEADER_NEW.clone(), hv.clone());
response.headers_mut().insert(REQUEST_ID_HEADER.clone(), hv);
```

- [ ] **Step 3: Update `chat/mod.rs` -- dual-emit session header**

Find `HeaderName::from_static("x-rcr-session")` and add `x-solvela-session` alongside it.

- [ ] **Step 4: Update `chat/provider.rs` -- dual-accept fallback-preference, dual-emit fallback**

For the input header:
```rust
// Accept both x-solvela-fallback-preference and x-rcr-fallback-preference
headers.get("x-solvela-fallback-preference")
    .or_else(|| headers.get("x-rcr-fallback-preference"))
```

For the output `x-rcr-fallback` header, emit both `x-solvela-fallback` and `x-rcr-fallback`.

- [ ] **Step 5: Update `proxy.rs` -- forward request ID with new header name too**

Find `upstream_req = upstream_req.header("x-rcr-request-id", rid.as_str())` and add the new header:
```rust
upstream_req = upstream_req.header("x-solvela-request-id", rid.as_str());
upstream_req = upstream_req.header("x-rcr-request-id", rid.as_str());
```

- [ ] **Step 6: Update `lib.rs` CORS exposed headers list**

The CORS config in `lib.rs` exposes header names. Add the `x-solvela-*` variants alongside the existing `x-rcr-*` ones.

- [ ] **Step 7: Update integration tests**

In `crates/gateway/tests/integration.rs`, update assertions to check for `x-solvela-*` headers (primary) while also verifying backward-compat `x-rcr-*` headers are still present. For tests that send `x-rcr-debug: true`, also add tests with `x-solvela-debug: true`.

- [ ] **Step 8: Update unit tests in debug_headers.rs and request_id.rs**

Update existing unit tests to assert both header variants are present. Update `is_debug_enabled` tests to verify both `x-solvela-debug` and `x-rcr-debug` work.

- [ ] **Step 9: Update CI smoke test**

In `.github/workflows/ci.yml`, update the smoke test that checks for `x-rcr-request-id` to also verify `x-solvela-request-id`:
```bash
echo "$headers" | grep -qi "x-solvela-request-id"
```

Also update the debug header smoke test to send `X-Solvela-Debug: true`.

- [ ] **Step 10: Verify compilation and tests**

Run: `cargo check --all-targets && cargo test --all && cargo clippy --all-targets --all-features -- -D warnings`
Expected: All pass.

- [ ] **Step 11: Commit**

```bash
git add crates/gateway/ .github/workflows/ci.yml
git commit -m "feat: add x-solvela- HTTP headers with x-rcr- backward compatibility

Response headers now emit both x-solvela-* and x-rcr-* variants.
Input headers (debug, fallback-preference) accept both prefixes.
Integration tests and CI smoke tests updated."
```

---

### Task 5: SDK Package Renames

Rename SDK packages. This is mostly config file changes.

**Files:**
- Modify: `sdks/python/pyproject.toml`
- Rename: `sdks/python/rustyclawrouter/` -> `sdks/python/solvela/`
- Modify: `sdks/python/tests/*.py` (import statements)
- Modify: `sdks/typescript/package.json`
- Modify: `sdks/typescript/src/client.ts`
- Modify: `sdks/go/go.mod`
- Modify: `sdks/go/*.go` (any Solvela references)
- Modify: `sdks/mcp/package.json`
- Modify: `sdks/mcp/src/client.ts`

**Package rename mapping:**
| Old | New |
|---|---|
| `rustyclawrouter` (Python) | `solvela-sdk` |
| `sdks/python/rustyclawrouter/` (directory) | `sdks/python/solvela/` |
| `@rustyclawrouter/sdk` (TypeScript) | `@solvela/sdk` |
| `github.com/rustyclawrouter/sdk-go` (Go) | `github.com/solvela/sdk-go` |
| `@rustyclawrouter/mcp` (MCP) | `@solvela/mcp-server` |
| `rcr-mcp` (MCP bin) | `solvela-mcp` |

- [ ] **Step 1: Rename Python SDK package**

In `sdks/python/pyproject.toml`:
```toml
[project]
name = "solvela-sdk"
description = "Python SDK for Solvela -- AI agent payments with USDC on Solana"
```

Rename directory:
```bash
mv sdks/python/rustyclawrouter sdks/python/solvela
```

Update all Python imports in `sdks/python/tests/*.py`:
- `from rustyclawrouter import` -> `from solvela import`
- `from rustyclawrouter.client import` -> `from solvela.client import`
- etc.

Update `sdks/python/solvela/__init__.py` if it references the old name.

- [ ] **Step 2: Rename TypeScript SDK**

In `sdks/typescript/package.json`:
```json
{
  "name": "@solvela/sdk",
  "description": "TypeScript SDK for Solvela -- AI agent payments with USDC on Solana"
}
```

Update any `Solvela` references in `sdks/typescript/src/client.ts`.

- [ ] **Step 3: Rename Go SDK**

In `sdks/go/go.mod`:
```
module github.com/solvela/sdk-go
```

Update any `rustyclawrouter` references in Go source files (`sdks/go/*.go`).

- [ ] **Step 4: Rename MCP SDK**

In `sdks/mcp/package.json`:
```json
{
  "name": "@solvela/mcp-server",
  "description": "MCP server for Solvela -- AI agents pay for LLM calls with USDC on Solana",
  "bin": {
    "solvela-mcp": "dist/index.js"
  }
}
```

Update any `rustyclawrouter` references in `sdks/mcp/src/*.ts`.

- [ ] **Step 5: Run SDK tests where possible**

Run: `cd sdks/go && go build ./...`
Run: `cd sdks/typescript && npm run build` (if node_modules present)
Run: `cd sdks/python && python -c "import solvela"` (if venv present)

Note: SDK tests may need network/dependencies. Verify what you can.

- [ ] **Step 6: Commit**

```bash
git add sdks/
git commit -m "refactor: rename SDK packages to solvela

Python: rustyclawrouter -> solvela-sdk (dir: solvela/)
TypeScript: @rustyclawrouter/sdk -> @solvela/sdk
Go: github.com/rustyclawrouter/sdk-go -> github.com/solvela/sdk-go
MCP: @rustyclawrouter/mcp -> @solvela/mcp-server"
```

---

### Task 6: Infrastructure Configs

Update Fly.io, Docker, and CI configs.

**Files:**
- Modify: `fly.toml`
- Modify: `docker-compose.yml`
- Modify: `Dockerfile`
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Update fly.toml**

```toml
app = "solvela-gateway"
```

- [ ] **Step 2: Update docker-compose.yml**

```yaml
# Solvela local development stack
services:
  postgres:
    container_name: solvela_postgres
    environment:
      POSTGRES_USER: solvela
      POSTGRES_PASSWORD: solvela_dev_password
      POSTGRES_DB: solvela
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U solvela -d solvela"]

  redis:
    container_name: solvela_redis
```

Update the comment at top:
```yaml
# DATABASE_URL=postgres://solvela:solvela_dev_password@localhost:5432/solvela
```

- [ ] **Step 3: Update Dockerfile**

```dockerfile
RUN cargo build --release --bin solvela-gateway 2>/dev/null || true
# ...
RUN cargo build --release --bin solvela-gateway

COPY --from=builder /app/target/release/solvela-gateway .

CMD ["./solvela-gateway"]
```

- [ ] **Step 4: Update CI workflow**

In `.github/workflows/ci.yml`:

Smoke test job:
```yaml
services:
  postgres:
    env:
      POSTGRES_USER: solvela
      POSTGRES_PASSWORD: solvela
      POSTGRES_DB: solvela_test
    options: >-
      --health-cmd "pg_isready -U solvela"
```

Start gateway step:
```yaml
DATABASE_URL="postgres://solvela:solvela@localhost:5432/solvela_test" \
SOLVELA_DEV_BYPASS_PAYMENT=true \
SOLVELA_ADMIN_TOKEN=ci-test-token \
./target/release/solvela-gateway &
```

Docker build step:
```yaml
tags: solvela-gateway:ci
```

- [ ] **Step 5: Verify Docker build**

Run: `docker build -t solvela-gateway:test .`
Expected: Builds successfully.

- [ ] **Step 6: Commit**

```bash
git add fly.toml docker-compose.yml Dockerfile .github/workflows/ci.yml
git commit -m "refactor: rename infrastructure configs from rustyclawrouter to solvela

fly.toml app name, docker-compose DB credentials and container names,
Dockerfile binary name, CI workflow service names and env vars."
```

---

### Task 7: Documentation Update

Update all documentation to reference Solvela instead of Solvela.

**Files:**
- Modify: `CLAUDE.md`
- Modify: `HANDOFF.md`
- Modify: `README.md`
- Modify: `CHANGELOG.md` (add entry, don't rewrite history)
- Modify: `docs/book/src/**/*.md` (all book docs)
- Modify: `docs/product/*.md`
- Modify: `sdks/*/README.md`
- Modify: Code comments mentioning "Solvela" or "rustyclaw"

**Important:** Plan documents in `docs/plans/` reference historical context. Update only where the old name would be confusing; don't rewrite git history references.

- [ ] **Step 1: Update CLAUDE.md**

Replace all occurrences of "Solvela" with "Solvela" in project description, binary names, env var examples, and architecture sections. Update:
- "Solvela is a Solana-native..." -> "Solvela is a Solana-native..."
- Binary name references: `rustyclawrouter` -> `solvela-gateway`, `rcr` -> `solvela`
- Env var references: `RCR_` -> `SOLVELA_` (note both are accepted)
- Crate name references: `rustyclaw-protocol` -> `solvela-protocol`, `router` -> `solvela-router`
- Package references: `rustyclawrouter-cli` -> `solvela-cli`
- Docker/CI references
- The CLAUDE.md in `.claude/worktrees/enterprise-features/` -- leave alone (worktree)

- [ ] **Step 2: Update README.md**

Replace project name, badges, installation instructions, binary names. Keep the same structure.

- [ ] **Step 3: Update HANDOFF.md**

Replace project name in header and references.

- [ ] **Step 4: Add CHANGELOG.md entry**

Add at the top:
```markdown
## [Unreleased]

### Changed
- **BREAKING**: Rebranded from Solvela to Solvela
  - Crate names: `rustyclaw-protocol` -> `solvela-protocol`, `router` -> `solvela-router`, etc.
  - Binary names: `rustyclawrouter` -> `solvela-gateway`, `rcr` -> `solvela`
  - Env vars: `SOLVELA_*` prefix (legacy `RCR_*` still accepted with deprecation warning)
  - HTTP headers: `x-solvela-*` prefix (legacy `x-rcr-*` still emitted for backward compatibility)
  - SDK packages renamed (see SDK READMEs)
  - Docker/Fly.io configs updated
```

- [ ] **Step 5: Update docs/book markdown files**

Global search-and-replace across `docs/book/src/`:
- "Solvela" -> "Solvela"
- "`rustyclawrouter`" -> "`solvela-gateway`"
- "`rcr`" (when referring to CLI binary) -> "`solvela`"
- "`RCR_`" -> "`SOLVELA_`" (in config examples)
- "`x-rcr-`" -> "`x-solvela-`" (in header examples)

Also update `docs/book/book.toml` if it contains project name.

- [ ] **Step 6: Update docs/product markdown files**

Same replacements in `docs/product/faq.md`, `docs/product/how-it-works.md`, `docs/product/use-cases.md`, `docs/product/regulatory-position.md`.

- [ ] **Step 7: Update SDK READMEs**

Replace "Solvela" with "Solvela" in:
- `sdks/python/README.md`
- `sdks/typescript/README.md`
- `sdks/mcp/README.md`
- `sdks/go/README.md` (if it exists)

- [ ] **Step 8: Update dashboard UI branding**

Replace "Solvela" with "Solvela" in:
- `dashboard/src/app/layout.tsx` (line 9: `title: "Solvela Dashboard"` -> `title: "Solvela Dashboard"`)
- `dashboard/src/app/settings/page.tsx` (lines 382, 387: API endpoint description)
- `dashboard/src/components/layout/sidebar.tsx` (line 53: brand text)
- `dashboard/src/components/layout/shell.tsx` (line 33: brand text)
- `dashboard/src/__tests__/api.test.ts` (lines 70, 111: platform name assertions -> `"Solvela"`)

- [ ] **Step 9: Update code comments**

Search for remaining "Solvela" or "rustyclaw" in Rust source files and update comments:
- `crates/protocol/src/lib.rs` doc comment
- `crates/gateway/src/config.rs` comments about `RCR_SOLANA__FEE_PAYER_KEY_2`
- `programs/escrow/Cargo.toml` comments
- `crates/gateway/src/routes/orgs/mod.rs` comment referencing `rcr_k_...` -> `solvela_k_...`
- Any other straggling comments

Run: `grep -rn "Solvela\|rustyclaw" crates/ programs/ --include="*.rs" --include="*.toml"` and fix remaining hits.

- [ ] **Step 10: Update integrations/openclaw references**

Check `integrations/openclaw/` for Solvela references and update to Solvela.

- [ ] **Step 11: Commit**

```bash
git add CLAUDE.md HANDOFF.md README.md CHANGELOG.md docs/ sdks/ crates/ programs/ integrations/
git commit -m "docs: rebrand documentation from Solvela to Solvela

Update project name, binary names, env var prefixes, and header prefixes
across all documentation, READMEs, code comments, and book pages."
```

---

### Task 8: Prometheus Metrics Rename

Rename Prometheus metric names from `rcr_*` to `solvela_*`.

**Files:**
- Modify: `crates/gateway/src/middleware/metrics.rs`
- Modify: `crates/gateway/src/balance_monitor.rs`
- Modify: `.github/workflows/ci.yml` (smoke test checks for metric name)

**Metric mapping:**
| Old | New |
|---|---|
| `rcr_requests_total` | `solvela_requests_total` |
| `rcr_request_duration_seconds` | `solvela_request_duration_seconds` |
| `rcr_active_requests` | `solvela_active_requests` |
| `rcr_fee_payer_balance_sol` | `solvela_fee_payer_balance_sol` |

- [ ] **Step 1: Update metrics.rs**

Replace all `"rcr_` metric name prefixes with `"solvela_` in:
- `metrics::gauge!("rcr_active_requests")` -> `metrics::gauge!("solvela_active_requests")`
- `metrics::histogram!("rcr_request_duration_seconds"` -> `metrics::histogram!("solvela_request_duration_seconds"`
- `metrics::counter!("rcr_requests_total"` -> `metrics::counter!("solvela_requests_total"`

Also update doc comments referencing these metric names.

- [ ] **Step 2: Update balance_monitor.rs**

Replace `"rcr_fee_payer_balance_sol"` with `"solvela_fee_payer_balance_sol"`.

- [ ] **Step 3: Update CI smoke test**

In `.github/workflows/ci.yml`, change:
```bash
echo "$body" | grep -q "rcr_requests_total"
```
to:
```bash
echo "$body" | grep -q "solvela_requests_total"
```

- [ ] **Step 4: Verify**

Run: `cargo check --all-targets && cargo test --all`

- [ ] **Step 5: Commit**

```bash
git add crates/gateway/src/middleware/metrics.rs crates/gateway/src/balance_monitor.rs \
  .github/workflows/ci.yml
git commit -m "refactor: rename Prometheus metrics from rcr_ to solvela_ prefix"
```

---

### Task 9: API Key Prefix Rename

Rename the API key prefix from `rcr_k_` to `solvela_k_`, with dual-accept for existing keys.

**Files:**
- Modify: `crates/gateway/src/orgs/queries.rs`
- Modify: `crates/gateway/src/orgs/models.rs` (test fixtures)
- Modify: `crates/gateway/src/middleware/api_key.rs`
- Modify: `crates/gateway/src/routes/orgs/mod.rs` (if references exist)

- [ ] **Step 1: Update key generation in queries.rs**

Change `generate_api_key()`:
```rust
format!("solvela_k_{}", hex::encode(bytes))
```

Update the prefix constant comment and length calculation.

- [ ] **Step 2: Update key detection in api_key.rs**

Accept both prefixes:
```rust
if auth.starts_with("solvela_k_") || auth.starts_with("rcr_k_") {
```

- [ ] **Step 3: Update test fixtures in models.rs**

Update test API key strings from `"rcr_k_supersecretkey"` to `"solvela_k_supersecretkey"`.

- [ ] **Step 4: Update test assertions in queries.rs**

Update:
```rust
key.starts_with("solvela_k_"),
"key should start with 'solvela_k_', got: {key}"
```

- [ ] **Step 5: Verify**

Run: `cargo test --all`

- [ ] **Step 6: Commit**

```bash
git add crates/gateway/src/orgs/ crates/gateway/src/middleware/api_key.rs
git commit -m "refactor: rename API key prefix from rcr_k_ to solvela_k_

New keys are generated with solvela_k_ prefix. Existing rcr_k_ keys
are still accepted for backward compatibility."
```

---

### Task 10: Final Verification

Full test suite across all components.

- [ ] **Step 1: Full Rust verification**

Run:
```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```
Expected: All pass, zero warnings.

- [ ] **Step 2: Escrow program verification**

Run:
```bash
cargo check --manifest-path programs/escrow/Cargo.toml
cargo test --manifest-path programs/escrow/Cargo.toml
```
Expected: All pass.

- [ ] **Step 3: Dashboard tests**

Run: `npm --prefix dashboard test`
Expected: All pass.

- [ ] **Step 4: Go SDK verification**

Run: `cd sdks/go && go build ./...`
Expected: Builds cleanly.

- [ ] **Step 5: Grep for remaining old references**

Run:
```bash
grep -rn "Solvela\|rustyclaw\|rustyclawrouter" \
  --include="*.rs" --include="*.toml" --include="*.ts" --include="*.py" \
  --include="*.go" --include="*.json" --include="*.yml" --include="*.yaml" \
  --include="*.md" --include="*.toml" --include="*.sh" \
  crates/ programs/ sdks/ dashboard/src/ config/ .github/ \
  Cargo.toml fly.toml docker-compose.yml Dockerfile \
  .env.example CLAUDE.md HANDOFF.md README.md CHANGELOG.md
```

Expected: Zero matches (excluding historical plan documents, worktrees, and Cargo.lock which regenerates).

Also check for straggling `rcr_` prefixes that should now be `solvela_` (excluding dual-accept fallback code):
```bash
grep -rn '"RCR_' crates/ --include="*.rs" | grep -v "env_dual\|fallback\|or_else\|rcr_name"
```

- [ ] **Step 6: Docker build verification**

Run: `docker build -t solvela-gateway:verify .`
Expected: Builds successfully.

- [ ] **Step 7: Final commit (if any fixups needed)**

If the grep in Step 5 found stragglers, fix them and commit:
```bash
git commit -m "fix: clean up remaining rustyclaw references missed in rebrand"
```
