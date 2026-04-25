> **⚠️ SUPERSEDED 2026-04-19** — This advisory draft was written before I'd seen `docs/runbooks/phase-4-publish.md`, which is the canonical V1.0 publish runbook and supersedes this file.
> Key differences vs reality:
> - The Rust CLI ships via **GitHub Releases + `@solvela/cli` npm meta-package + per-platform `@solvela/cli-{linux,darwin,win32}-*` binaries**, NOT via crates.io. That sidesteps the `x402` crate-name collision entirely — good call.
> - V1.0 scope is **npm-only**. PyPI (`solvela-sdk`) and Go SDK are deferred beyond V1.0.
> - The publish order, propagation waits, and `optionalDependencies` sequencing are more detailed in `phase-4-publish.md`. Use that.
>
> **Kept here for:** the crates.io competitive-research notes (still valid — see `docs/strategy/2026-04-19-rust-x402-landscape.md` for the full analysis), and optional rollback/verification ideas not covered in phase-4.

# SDK + CLI + Plugin Publish Runbook (advisory draft — superseded)

> **Fire condition:** OpenClaw plugin smoke test passes with a real paid call on devnet.
> **Order matters.** Registries have different unpublish policies — publish the hardest-to-reverse first so failures surface early and cheaply.
> **Scope:** Solvela ecosystem only (Sky64 is not in this runbook).
> **Date drafted:** 2026-04-19

---

## 0 — Pre-flight gates (do not skip)

Run through this list BEFORE touching any registry. Every item is one-line fixable now, expensive to fix after publish.

### Version alignment

Every artifact ships as `0.1.0` except the OpenClaw plugin which is currently at `1.0.0-draft` — align to `0.1.0` to match the rest, or graduate all five to `1.0.0` if you want a cleaner story. My recommendation: **ship as `0.1.0` across the board.** You're signaling "usable, stable API likely, expect polish." `1.0.0` commits you to SemVer breakage rules you probably don't want yet.

**Fix:** `sdks/openclaw-provider/package.json` → `"version": "0.1.0"` before publish.

### Private flags on npm

Three npm packages currently have `"private": true`. npm refuses to publish private packages. Must flip.

```bash
# Packages to un-private:
#   sdks/typescript/package.json         → @solvela/sdk
#   sdks/mcp/package.json                → @solvela/mcp-server
#   sdks/openclaw-provider/package.json  → @solvela/openclaw-provider
#
# Keep private (internal only):
#   sdks/signer-core/package.json        → @solvela/signer-core
```

**Decide:** is `@solvela/signer-core` an internal-only workspace package (stays private, bundled into consumers), or a public shared lib? If consumers like `@solvela/sdk` or `@solvela/openclaw-provider` depend on it at runtime, **it must be public** or you ship broken packages.

Check with:
```bash
grep -l "signer-core" /home/kennethdixon/projects/solvela/sdks/*/package.json
```

If it's a runtime dep anywhere → un-private it, publish it FIRST in the npm order.

### License, README, repo links on every package

Each `package.json` / `pyproject.toml` / `Cargo.toml` needs:
- `license: MIT` (already set on most)
- `repository`, `homepage`, `bugs` URLs pointing to the public GitHub repo
- `keywords` (discoverability)
- README.md with a working quickstart — **paste and run on a fresh machine**

### Go module path sanity check

`go.mod` says `module github.com/solvela/sdk-go`. This is only valid if a GitHub repo literally exists at `github.com/solvela/sdk-go`. If the repo is actually at `github.com/solvela-ai/solvela-go` (per handoff naming), the module path is wrong and `go get` will fail.

**Verify with:**
```bash
curl -sI https://github.com/solvela/sdk-go | head -1
```

If 404 → fix the module path in `go.mod` before tagging.

### Rust publishing is more involved than the rest

`solvela-cli` depends on workspace crates `x402` and `solvela-router`. **crates.io rejects publishing a crate with path-dependencies unpublished on crates.io.**

This means the Rust publish chain is:

1. `solvela-protocol` (no internal deps) — first
2. `x402` (depends on protocol) — second
3. `solvela-router` (depends on protocol) — third
4. `solvela-cli` (depends on x402, router) — last

For each workspace crate you publish, the `Cargo.toml` must use **versioned** dependencies, not `path = "..."`. Cargo.toml pattern:

```toml
# WRONG — path-only, won't publish
x402 = { path = "../x402" }

# CORRECT — both path (for local dev) and version (for publish)
x402 = { path = "../x402", version = "0.1.0" }
```

Check each `crates/*/Cargo.toml` has both.

**Alternative if you don't want to publish four crates:** publish only `solvela-cli` with the internal crates **vendored/inlined** as modules. Cleaner registry but breaks the open-source story. Recommendation: take the hit once, publish all four, you get a cleaner ecosystem.

### Dry runs on every registry

Each registry has a dry-run mode. Use it. Every bug surfaced in dry-run is a bug not published.

```bash
# npm
npm publish --dry-run --access public

# PyPI via twine
python -m build
python -m twine check dist/*

# crates.io
cargo publish --dry-run -p solvela-protocol
```

---

## 1 — Publish order (hardest-to-reverse first)

Rationale: crates.io cannot truly unpublish (30-day yank window, can never reuse the version). PyPI can yank but not re-upload. npm can unpublish within 72 hours. Go is just a tag. MCP is npm, so after npm is stable. Order:

**1. crates.io (Rust) → 2. PyPI (Python) → 3. npm (TS SDK + AI SDK provider) → 4. Go mod tag → 5. MCP server (npm) → 6. OpenClaw provider (npm)**

---

## 2 — Step 1: crates.io (Rust)

### Prereqs

- Logged in: `cargo login <token>` (get token from https://crates.io/me)
- You own the crate names: `solvela-protocol`, `x402`, `solvela-router`, `solvela-cli`. If the name `x402` is already taken on crates.io, check now: `cargo search x402`. You may have to rename to `solvela-x402`.

### Publish chain

```bash
cd /home/kennethdixon/projects/solvela

# Dry-run each first
cargo publish --dry-run -p solvela-protocol
cargo publish --dry-run -p x402
cargo publish --dry-run -p solvela-router
cargo publish --dry-run -p solvela-cli

# If all dry-runs pass, publish in order (wait ~60s between each for index propagation)
cargo publish -p solvela-protocol
sleep 60
cargo publish -p x402
sleep 60
cargo publish -p solvela-router
sleep 60
cargo publish -p solvela-cli
```

### Verify

```bash
# From a tmp dir, install the CLI fresh
cd /tmp && mkdir solvela-install-test && cd solvela-install-test
cargo install solvela-cli
solvela --version    # Should print the version you just published
solvela doctor       # Should complete cleanly
```

**If verify fails:** you cannot unpublish. You can yank (`cargo yank`). Bump patch (0.1.1) and republish. This is why dry-run matters.

---

## 3 — Step 2: PyPI

### Prereqs

- Account on https://pypi.org (and https://test.pypi.org — worth testing there first)
- API token saved to `~/.pypirc`:

```ini
[pypi]
  username = __token__
  password = pypi-<your token>

[testpypi]
  username = __token__
  password = pypi-<your test token>
  repository = https://test.pypi.org/legacy/
```

### Publish

```bash
cd /home/kennethdixon/projects/solvela/sdks/python

# Clean + build
rm -rf dist/ build/ *.egg-info
python -m build

# Validate
python -m twine check dist/*

# Test on testpypi first (optional but recommended)
python -m twine upload --repository testpypi dist/*
pip install --index-url https://test.pypi.org/simple/ solvela-sdk

# Production
python -m twine upload dist/*
```

### Verify

```bash
# Fresh venv
python -m venv /tmp/solvela-pypi-test && source /tmp/solvela-pypi-test/bin/activate
pip install solvela-sdk
python -c "import solvela; print(solvela.__version__)"
deactivate
```

---

## 4 — Step 3: npm (TS SDK + AI SDK provider)

### Prereqs

- Logged in: `npm whoami` → confirms logged-in user owns the `@solvela` scope
- If scope doesn't exist: create the org on npmjs.com first (`@solvela` organization, free tier ok for public packages)

### Un-private the packages

Before publishing, in each `package.json` that will be public:

```json
{
  "private": false,   // or delete the field entirely
  ...
}
```

Do this for `@solvela/sdk`, `@solvela/mcp-server`, `@solvela/openclaw-provider`. Keep `@solvela/signer-core` private ONLY if nothing consumes it at runtime.

### Publish SDK first (consumers depend on it)

```bash
cd /home/kennethdixon/projects/solvela/sdks/typescript
npm run build     # whatever your build script is
npm publish --access public --dry-run
npm publish --access public
```

### Publish AI SDK provider

```bash
cd /home/kennethdixon/projects/solvela/sdks/ai-sdk-provider
npm run build
npm publish --access public --dry-run
npm publish --access public
```

### Verify

```bash
# Fresh npm install in a scratch dir
cd /tmp && mkdir solvela-npm-test && cd solvela-npm-test
npm init -y
npm install @solvela/sdk @solvela/ai-sdk-provider
node -e "console.log(require('@solvela/sdk/package.json').version)"
```

---

## 5 — Step 4: Go module tag

Go "publish" is just a git tag. The Go proxy (`proxy.golang.org`) picks it up on first `go get`.

### Pre-check module path

Confirm `sdks/go/go.mod` module line matches the actual GitHub repo URL. If the Go SDK is vendored inside this monorepo at `sdks/go/`, and the canonical module is meant to be a standalone repo like `github.com/solvela-ai/solvela-go`, you need to:

**Option A:** Keep SDK in this monorepo — module path becomes `github.com/<owner>/solvela/sdks/go`. Less clean.

**Option B:** Split `sdks/go/` to its own public repo `github.com/solvela-ai/solvela-go` (recommended). Then tag there.

### Tag (assuming standalone repo)

```bash
cd /path/to/solvela-go-repo
git tag v0.1.0
git push origin v0.1.0
```

### Verify

```bash
# Fresh module dir
cd /tmp && mkdir solvela-go-test && cd solvela-go-test
go mod init test
go get github.com/solvela-ai/solvela-go@v0.1.0
go run -e 'package main; import _ "github.com/solvela-ai/solvela-go"; func main() {}'
```

Wait 2–3 minutes after the tag push for the Go proxy to cache it. If `go get` errors with "version not found" that's usually proxy lag, retry.

---

## 6 — Step 5: MCP server (npm)

Same mechanics as step 4, but separate because it's a separate consumer-visible package and worth its own smoke test.

```bash
cd /home/kennethdixon/projects/solvela/sdks/mcp
# confirm private: false
npm run build
npm publish --access public --dry-run
npm publish --access public
```

### Verify

```bash
# The MCP server should be runnable via npx with no install
npx -y @solvela/mcp-server --version

# And installable via solvela CLI (which you just published in step 1):
solvela mcp install --host=claude-code
```

That second command is your end-to-end smoke test — it exercises the CLI, the MCP server, and the npm install path all together.

---

## 7 — Step 6: OpenClaw provider (npm, last)

Last because it's brand-new and you want the other publishing muscle memory warm before you ship it.

```bash
cd /home/kennethdixon/projects/solvela/sdks/openclaw-provider
# confirm version: 0.1.0, private: false
npm run build
npm publish --access public --dry-run
npm publish --access public
```

### Verify

Actually install and use it through OpenClaw. Make a real paid call on devnet. That's the smoke test.

---

## 8 — Rollback: what to do if something goes wrong

| Registry | Undo option | Reality |
|---|---|---|
| **crates.io** | `cargo yank --vers 0.1.0 <crate>` | Existing users keep it; no new installs. Version number is burned — next publish must be 0.1.1 |
| **PyPI** | Yank via web UI | Same semantic as crates.io. Re-upload under 0.1.0 is **not allowed** |
| **npm** | `npm unpublish @solvela/sdk@0.1.0` | Only works within 72 hours. After that, only yank via npm support |
| **Go** | Delete git tag, force-push | Proxy may have cached. Retag `v0.1.1` is safest |

**The right answer to almost every failure: bump patch version and republish.** Don't fight the registries.

---

## 9 — Post-publish: tag the release in this repo

```bash
cd /home/kennethdixon/projects/solvela
git tag -a v0.1.0-public-sdks -m "Public SDK release: crates.io, PyPI, npm, Go, MCP"
git push origin v0.1.0-public-sdks
```

Update `HANDOFF.md` "What's NOT Done → SDK publishing" → move to "What's Built."

---

## 10 — The one-shot checklist

A compressed view for the day-of execution:

- [ ] OpenClaw plugin smoke test: PASS
- [ ] Pre-flight: versions aligned to 0.1.0 (OpenClaw was 1.0.0-draft)
- [ ] Pre-flight: `private: true` removed from 3 npm packages
- [ ] Pre-flight: `signer-core` publish/keep-private decision made
- [ ] Pre-flight: Go module path matches actual repo URL
- [ ] Pre-flight: Rust workspace crates have version + path fields
- [ ] Pre-flight: Every package has README quickstart that runs on fresh machine
- [ ] Pre-flight: `cargo publish --dry-run` passes for all 4 crates
- [ ] Pre-flight: `npm publish --dry-run` passes for all 4 packages
- [ ] Pre-flight: `twine check` passes for PyPI dist
- [ ] **Publish:** crates.io chain (protocol → x402 → router → cli)
- [ ] **Verify:** `cargo install solvela-cli` works from clean `/tmp`
- [ ] **Publish:** PyPI (`solvela-sdk`)
- [ ] **Verify:** `pip install solvela-sdk` works from clean venv
- [ ] **Publish:** npm `@solvela/sdk` + `@solvela/ai-sdk-provider`
- [ ] **Verify:** `npm install @solvela/sdk` works from scratch
- [ ] **Publish:** Go tag
- [ ] **Verify:** `go get github.com/.../solvela-go@v0.1.0` works
- [ ] **Publish:** `@solvela/mcp-server`
- [ ] **Verify:** `npx -y @solvela/mcp-server --version` works
- [ ] **Publish:** `@solvela/openclaw-provider`
- [ ] **Verify:** real paid devnet call through OpenClaw
- [ ] Tag repo: `v0.1.0-public-sdks`
- [ ] Update HANDOFF.md
- [ ] Announce (see `2026-04-19-announcement.md`)

Total elapsed time if nothing breaks: **2–4 hours**.

Total elapsed time with one Rust workspace dep issue: **6–10 hours**.

Budget for the second case and be pleasantly surprised.
