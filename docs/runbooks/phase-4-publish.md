# Phase 4 Publish Runbook

Gated on user green-light after private real-user testing. Do NOT run any step without explicit user approval.

## Scope (V1.0)

**In scope:** `@solvela/signer-core`, `@solvela/sdk`, `@solvela/mcp-server`, `@solvela/openclaw-provider`, `@solvela/cli` (+ platform packages), `solvela-cli` Rust binary via GitHub Releases.

**Platform coverage:** `linux-x64`, `win32-x64`, `darwin-x64`, `darwin-arm64`. Out of scope for V1.0: `linux-arm64` (Raspberry Pi / Graviton), `linux-musl` (Alpine). Add in V1.1.

**Version skew (deliberate):** Rust workspace is `0.1.0`; npm wrappers are `1.0.0-draft`. The npm CLI meta-package (`@solvela/cli`) and Rust CLI (`solvela-cli`) are versioned independently — wrapper version reflects UX maturity, binary version reflects code maturity. Bump workspace `Cargo.toml` to `1.0.0` if you want these synchronized before tag.

## Pre-flight verification

Before removing any `"private": true` gates, confirm:

- [ ] Tag is GPG-signed (`git tag -v v1.0.0` succeeds). The release workflow does NOT verify this today — verify manually.
- [ ] All passing tests across all packages:
  ```bash
  (cd sdks/signer-core && npm test)
  (cd sdks/typescript && npm test)
  (cd sdks/mcp && npm test)
  (cd sdks/openclaw-provider && npm test)
  (cd sdks/ai-sdk-provider && npm test)
  cargo test --workspace
  ```
- [ ] Local smoke test passes: `bash sdks/cli-npm/scripts/verify-release.sh`
- [ ] CHANGELOG updated for V1.0.0
- [ ] npm org tokens configured in GitHub Secrets (`NPM_TOKEN`)
- [ ] `id-token: write` permission added to `.github/workflows/release.yml` (required for `npm publish --provenance`)
- [ ] GPG signing key configured on the operator's machine
- [ ] You have reviewed each `docs/launch-drafts/*.md` + `docs/launch-drafts/anthropic-mcp-registry.json` and they are ready to submit after npm publish lands
- [ ] `@solvela/ai-sdk-provider` is already public at a stable version — decide whether V1.0 requires a bump alongside this launch

## Remove publish gates — verification

Remove `"private": true` from every `package.json` listed below. After each removal, grep to confirm the current state:

```bash
cd /home/kennethdixon/projects/solvela
# Expect ZERO matches in sdks/ before proceeding:
grep -rn '"private": true' \
  sdks/signer-core/package.json \
  sdks/typescript/package.json \
  sdks/mcp/package.json \
  sdks/openclaw-provider/package.json \
  sdks/cli-npm/package.json \
  sdks/cli-npm/platforms/*/package.json
```

That's 9 `package.json` files total. A single missed file leaves part of the ecosystem un-publishable and breaks the meta-package's `optionalDependencies` resolution.

## Swap `file:` deps → real version ranges

Before publishing, replace `"file:../..."` references in `package.json` dependency blocks with real version specifiers:

- `sdks/mcp/package.json`: `@solvela/sdk`, `@solvela/signer-core` → `"^1.0.0"`
- `sdks/openclaw-provider/package.json`: same
- `sdks/ai-sdk-provider/package.json`: same (if references any workspace dep this way)

Otherwise npm publish will fail with "Cannot install from file: in a published package."

## Publish order (dependencies first)

Run each step in a clean subshell. Fail fast on any non-zero exit:

```bash
#!/usr/bin/env bash
set -euo pipefail

REPO="$(git rev-parse --show-toplevel)"

# 1. signer-core (no workspace deps)
(cd "$REPO/sdks/signer-core"       && npm publish --access public --provenance)

# 2. @solvela/sdk (no workspace deps)
(cd "$REPO/sdks/typescript"        && npm publish --access public --provenance)

# Wait for registry propagation — npm CDN can take minutes
echo "Waiting 5 minutes for signer-core + sdk to propagate..."
sleep 300
npm view @solvela/signer-core version  # must succeed
npm view @solvela/sdk version          # must succeed

# 3. mcp-server (deps: signer-core, sdk)
(cd "$REPO/sdks/mcp"               && npm publish --access public --provenance)

# 4. openclaw-provider (deps: signer-core, sdk)
(cd "$REPO/sdks/openclaw-provider" && npm publish --access public --provenance)

# 5. ai-sdk-provider (if bumping alongside)
# (cd "$REPO/sdks/ai-sdk-provider"  && npm publish --access public --provenance)

# 6. Tag the Rust release (triggers .github/workflows/release.yml — produces draft GitHub Release)
git tag -s v1.0.0 -m "Release v1.0.0"
git push origin v1.0.0

# Wait for the draft release to populate with all 4 platform binaries
echo "Waiting for release workflow to finish..."
gh run watch  # optional — interactive watch

# 7. Flip draft release → published after smoke-testing the downloaded binaries
gh release edit v1.0.0 --draft=false

# 8. Download the 4 prebuilt binaries and place them in sdks/cli-npm/platforms/*/bin/
# (detailed in docs/runbooks/publish-cli-npm.md — create before running)

# 9. Publish each platform package (with its binary in place)
(cd "$REPO/sdks/cli-npm/platforms/linux-x64"    && npm publish --access public --provenance)
(cd "$REPO/sdks/cli-npm/platforms/win32-x64"    && npm publish --access public --provenance)
(cd "$REPO/sdks/cli-npm/platforms/darwin-x64"   && npm publish --access public --provenance)
(cd "$REPO/sdks/cli-npm/platforms/darwin-arm64" && npm publish --access public --provenance)

# Wait for ALL four platform packages to propagate
echo "Waiting 5 minutes for platform packages to propagate..."
sleep 300
for pkg in linux-x64 win32-x64 darwin-x64 darwin-arm64; do
  npm view "@solvela/cli-${pkg}" version
done

# 10. Publish meta-package LAST — its optionalDependencies must all resolve
(cd "$REPO/sdks/cli-npm" && npm publish --access public --provenance)
```

If the meta-package publishes before any platform package, `npm i -g @solvela/cli` on a user's machine fails the `optionalDependencies` resolution silently and the shim exits with `Could not resolve native binary`. That looks like a shim bug, not a propagation-lag. Wait 5 minutes after each `npm publish` batch and verify via `npm view` before the next batch.

## Post-publish

- [ ] Smoke test: `npm i -g @solvela/cli` on Linux, Windows, macOS x64, macOS arm64 (fresh machines or containers)
- [ ] `solvela --version` prints the correct Rust binary version
- [ ] `solvela mcp install --host=claude-code --dry-run` prints valid config
- [ ] Submit Anthropic MCP Registry entry (`docs/launch-drafts/anthropic-mcp-registry.json` → POST `api.anthropic.com/mcp-registry/v0/servers`)
- [ ] Open cursor.directory listing PR (`docs/launch-drafts/cursor-directory-submission.md`)
- [ ] Open OpenClaw docs PR (`docs/launch-drafts/openclaw-docs-pr.md`)
- [ ] Publish `solvela.ai/blog` post (`docs/launch-drafts/blog-post-solvela-ai.md`)
- [ ] Post HN Show (`docs/launch-drafts/hn-show-post.md`) — morning PT for best reach
- [ ] Post X thread (`docs/launch-drafts/x-thread.md`) — 1–2 hours after HN
- [ ] Send Solana Foundation grant update (`docs/launch-drafts/solana-foundation-grant-update.md`)
- [ ] Update CHANGELOG + create v1.0.0 GitHub Release notes (flip draft → published)

## Rollback

If a broken version ships to npm:

```bash
# Deprecate (preferred — npm does not support unpublish after 72h)
npm deprecate @solvela/cli@1.0.0 "broken release — use 1.0.1"
# Repeat for each affected package
npm deprecate @solvela/mcp-server@1.0.0 "broken release — use 1.0.1"
# Then ship a patch immediately
```

If a bad git tag was pushed:

```bash
# Delete local + remote tag
git tag -d v1.0.0
git push origin :refs/tags/v1.0.0
# Delete the GitHub Release (draft or published)
gh release delete v1.0.0 --yes
# Fix the underlying issue, then re-tag from the corrected commit
```

## Local smoke test (pre-publish verification)

Run the local verify script to confirm the shim works on the current host before tagging:

```bash
bash sdks/cli-npm/scripts/verify-release.sh
```

Expected output ends with:
```
[verify-release] SUCCESS: shim correctly resolved and executed the native binary.
```
