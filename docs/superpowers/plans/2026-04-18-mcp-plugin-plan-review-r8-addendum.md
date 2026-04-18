# R8 Windows Distribution — Research Addendum

**Parent review:** `2026-04-18-mcp-plugin-plan-review.md` §2.3
**Date:** 2026-04-18
**Research agents:** `document-specialist` (external SOTA) + `architect` (repo state + peer patterns), parallel
**Verdict:** **Cross-compile via `cargo-dist` + distribute through npm `optionalDependencies`.** The plan's proposed JS wrapper should be deleted.

---

## TL;DR

**The research makes this an easy call, and it contradicts the plan's R8 mitigation.**

Three independent findings collapse the trade-off:

1. **The Solvela workspace already has zero Windows cross-compile landmines.** The classic Rust-for-Solana Windows pitfalls (`solana-sdk`, `openssl-sys`) were deliberately avoided. `reqwest` is pinned to `rustls-tls` at `Cargo.toml:33`. `ed25519-dalek` / `curve25519-dalek` are pinned pure-Rust. No `build.rs`, no `cmake`, no C-deps in the CLI tree. Windows cross-compile will almost certainly work first-try.
2. **The plan's R8 mitigation contains a logical contradiction.** Plan line 276 says the npm wrapper "shells out to the same JSON generators shared with the Rust `solvela mcp install` subcommand." If the Rust binary doesn't exist on Windows (which is the premise of R8), there is nothing to shell out to. The wrapper must either (a) reimplement the generators in JS (permanent drift), or (b) bundle/download the Rust binary (which IS the cross-compile option).
3. **The canonical pattern for Rust-CLI-via-npm is solved.** Turborepo, Biome, esbuild, swc, rolldown all ship prebuilt Rust/Go binaries through npm `optionalDependencies`. One Rust codebase, native performance, `npm install` UX, no SmartScreen friction. ~16 hours of setup, near-zero ongoing maintenance.

**Recommendation: replace T4-G with a hybrid plan** — cross-compile Rust via `cargo-dist`, then publish to npm as `@solvela/cli` with platform-specific optional-dep packages. The user gets `npm i -g @solvela/cli` on every OS, backed by a single Rust source of truth.

---

## Key evidence

### Dependency tree is clean (architect finding)

| Dep | Classic Windows risk | Actual state in this repo |
|---|---|---|
| `reqwest` | `openssl-sys` breaks Windows cross-compile | **Defused** — `Cargo.toml:33` uses `default-features = false, features = ["json", "stream", "rustls-tls"]` |
| `solana-sdk` | Historically hit `curve25519-dalek` SIMD issues + OpenSSL on Windows | **Absent** — `crates/x402/src/solana_types.rs:3-5` explicitly says these lightweight types exist to *avoid* depending on `solana-sdk`. `crates/cli/src/commands/solana_tx.rs:5` confirms the same. |
| `ed25519-dalek`, `curve25519-dalek` | SIMD intrinsics, platform-specific asm | Pinned `default-features = false, features = ["alloc"]` — pure Rust |
| `getrandom` | Sometimes needs WASM shims | Uses `BCryptGenRandom` on Windows (native) |
| `build.rs` / `cc` / `cmake` / `pkg-config` | Any of these can break cross-compile | **None** in CLI tree |
| `aws-lc-sys` / `ring` | Native assembly builds, notorious on Windows | `cmake` appears in `Cargo.lock` but `cargo tree -p solvela-cli -i cmake` returns empty — it's pulled only by the escrow program (out of workspace) |

**Bottom line:** this is one of the cleanest Rust CLI dependency trees for Windows cross-compile you'll find in the Solana ecosystem. R8 was risk-rated as if Windows were hard. It is not hard *for this codebase*.

### Current release infrastructure is empty

- Only three GitHub workflows exist: `ci.yml` (Ubuntu-only lint/test), `deploy.yml` (Fly.io), `ai-sdk-provider.yml` (TS SDK).
- No `release.yml`, no `cargo-dist` config, no `dist-workspace.toml`, no `[workspace.metadata.dist]` block.
- No `release-please` or similar automation.
- Install today is `cargo install --path crates/cli` (from `README.md:259`) — no prebuilt binaries for any platform.
- TypeScript SDK has no postinstall hooks or binary-download logic — so there is zero prior art in this repo for the hybrid pattern, but also no conflicting infra to work around.

### `cargo-dist` is still the right tool in 2026 (document-specialist finding)

- Latest release **v0.31.0 (Feb 2026)**, 271 total releases, active cadence through 2025.
- `cargo dist init` generates `.github/workflows/release.yml` covering Linux/macOS/Windows from a single config block.
- Native `windows-latest` runner is the canonical path in 2026 — **do not use `cross`** for Windows. Ripgrep's workflow confirms this.
- Annual maintenance: bump `cargo-dist-version`, re-run `dist init` to regenerate YAML. Basically set-and-forget.

### npm `optionalDependencies` > postinstall scripts

- pnpm (late-2024+) does not run postinstall scripts by default.
- Corporate CIs routinely pass `--ignore-scripts`.
- The proven pattern (turborepo, biome, esbuild, swc, rolldown):
  - Main package `@solvela/cli` declares `optionalDependencies: { "@solvela/cli-linux-x64": "1.0.0", "@solvela/cli-win32-x64": "1.0.0", "@solvela/cli-darwin-x64": "1.0.0", "@solvela/cli-darwin-arm64": "1.0.0" }`
  - Each platform package is minimal (just the binary + a `bin` entry)
  - Main package has a ~50-line JS shim that resolves `process.platform` + `process.arch` and execs the binary
  - npm/pnpm/yarn natively skip incompatible optional deps — no scripts needed
- Biome is actively **removing** its postinstall script in v2.0 in favor of optional-deps-only (per research).

### SmartScreen is a non-issue on the npm path

- SmartScreen triggers on downloaded-and-double-clicked `.exe` files. It does **not** trigger on binaries under `node_modules/.bin/` invoked by MCP hosts or `npx`.
- Code signing certs cost $100-400/year and no longer bypass SmartScreen instantly (since Aug 2024 EV reputation changes).
- **By shipping through npm, code signing becomes optional.** If you also offer raw GitHub Release downloads, you'd want to sign those eventually — but that's V1.1+, not blocking V1.

### Peer patterns

| Tool | Windows strategy | Relevant? |
|---|---|---|
| **Turborepo** | Rust binary via npm `optionalDependencies` | Direct prior art — same pattern we'd adopt |
| **Biome** | Rust binary via npm `optionalDependencies` (removing postinstall in v2) | Direct prior art |
| **esbuild / swc / rolldown** | Native binary via npm `optionalDependencies` | Same pattern |
| **Foundry** | WSL-only, no npm path | *Not* what we want — npm path is better UX |
| **Solana CLI (Agave)** | `sh -c curl` installer; `.exe` on GH Releases | Weak Windows story; we can do better |
| **Anchor** | Cargo-install only; no prebuilt Windows binaries | We can do better |

---

## Effort estimates (architect finding)

| Path | First release | Ongoing per release | Duplication risk |
|---|---|---|---|
| **A. Plan as-written** (JS wrapper reimplements generators) | 8–12h | 1–2h to keep JS + Rust generators in sync | **High, permanent** |
| **B. Cross-compile only** (`cargo-dist` + GH Releases) | 6–8h | ~30 min (tag & wait) | None (single codebase) |
| **C. Hybrid** (cross-compile + npm `optionalDeps`) | 12–16h | ~30 min (tag triggers both) | None (single codebase) |

Plan option A costs more up-front AND has ongoing drift cost. B is the cheapest. **C is the winner** — 4–8 extra hours buys the `npm i -g @solvela/cli` UX on every platform, which is what MCP users (Node developers) actually expect.

---

## Recommended plan amendments

Replace existing **§7 R8** and **T4-G** with:

> **R8 (revised):** Until the Rust `solvela` CLI is available on all platforms users need, Windows and some macOS users cannot run `solvela mcp install`. **Mitigation:** cross-compile the Rust CLI via `cargo-dist` and distribute via npm `optionalDependencies` using the turborepo/biome pattern. Dependency audit confirms the tree is already Windows-cross-compile-safe (no `solana-sdk`, `rustls-tls` only, no `build.rs`). Effort: ~16h to first release; ~30 min per subsequent release.

> **Phase 4 T4-G (revised):** Add `cargo-dist` config to workspace `Cargo.toml` (targets: `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`). Run `dist init` to generate `.github/workflows/release.yml`. Publish `@solvela/cli` meta-package to npm with platform-specific optional-dep packages (`@solvela/cli-<platform>-<arch>`) each containing the prebuilt binary. Add a JS shim that detects platform and execs the binary. Delete the proposed `@solvela/mcp-install` JS wrapper — the hybrid approach replaces it.

**Concrete sub-tasks for T4-G (revised):**

- T4-G.1: Add `[workspace.metadata.dist]` config block with four targets + `installers = ["shell", "powershell"]`.
- T4-G.2: Run `cargo dist init`, commit generated `release.yml`.
- T4-G.3: Add `[[bin]]` section in `crates/cli/Cargo.toml` if not already present; verify `cargo dist plan` produces four artifacts.
- T4-G.4: Create `sdks/cli-npm/` package scaffold with platform-specific optional deps and JS shim. Follow biome's `packages/cli/` layout as template.
- T4-G.5: First tagged release `v1.0.0`; verify all four binaries upload to GH Releases and npm auto-publishes the platform packages.
- T4-G.6: Smoke test: `npm i -g @solvela/cli` on Windows → run `solvela mcp install --host=claude-code` → verify it writes valid config.

---

## Risks of the new approach (being honest)

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| First Windows cross-compile hits an unknown dep issue | Low (clean tree) | Low | Fall back to `cross` or add a `#[cfg(windows)]` shim; punt to V1.1 if truly stuck |
| npm optional-deps behave differently across npm/pnpm/yarn versions | Low | Medium | Biome/turborepo solved this; copy their shim verbatim |
| `cargo-dist` breaking change between runs | Low | Low | Pin the version; re-run `dist init` on upgrade |
| GitHub Actions minute cost (macOS is 10× multiplier) | Low | Low | ~30 billed min per release; negligible |
| Adds ~4–8 hours to Phase 4 beyond original estimate | High | Low | Worth it — kills an entire class of future maintenance |

---

## What was ruled out

- **Pure JS wrapper (plan as-written):** logical contradiction + permanent drift + extra maintenance surface. Rejected.
- **Using `cross` for Windows:** unnecessary — `windows-latest` runner works natively. Rejected.
- **Code signing before V1:** SmartScreen is a non-issue on the npm install path; $100-400/yr cert is deferrable to V1.1+. Deferred.
- **postinstall download scripts only:** pnpm default behavior + corporate `--ignore-scripts` break it. Use `optionalDependencies` instead.

---

## Sources (document-specialist)

- [cargo-dist releases](https://github.com/axodotdev/cargo-dist/releases) — v0.31.0, Feb 2026
- [cargo-dist Rust quickstart](https://axodotdev.github.io/cargo-dist/book/quickstart/rust.html)
- [ripgrep release.yml](https://github.com/BurntSushi/ripgrep/blob/master/.github/workflows/release.yml) — canonical manual GHA matrix
- [reemus.dev: Rust cross-platform GHA (Dec 2025)](https://reemus.dev/tldr/rust-cross-compilation-github-actions)
- [Sentry Engineering: publishing binaries on npm](https://sentry.engineering/blog/publishing-binaries-on-npm)
- [esbuild platform-specific binaries](https://deepwiki.com/evanw/esbuild/6.2-platform-specific-binaries)
- [Biome postinstall removal issue](https://github.com/biomejs/biome/issues/4854)
- [Microsoft Q&A: SmartScreen and code signing](https://learn.microsoft.com/en-us/answers/questions/5760202/code-signing-impact-on-smartscreen-and-non-windows)

---

## Evidence index (architect)

- `Cargo.toml:33` — `reqwest` pinned to `rustls-tls`
- `Cargo.toml:39-40` — pure-Rust `ed25519-dalek` / `curve25519-dalek`
- `crates/cli/Cargo.toml` — no `build.rs`, no C deps, no `solana-sdk`
- `crates/x402/src/solana_types.rs:3-5` — deliberate `solana-sdk` avoidance
- `.github/workflows/ci.yml:11` — Ubuntu-only, no release workflow
- `README.md:259` — current install is `cargo install --path`
- Plan `line 276` — contradictory wrapper mitigation
- Plan `line 316` — R8 risk definition to revise

---

*End of addendum. Merge decisions into parent review §2.3 and amendment checklist item 4 when ready.*
