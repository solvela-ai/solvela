# Runbook: publishing `@solvela/signer-core` + `@solvela/mcp-server` to npm

Bootstrap (and ongoing) release procedure for the two-package MCP publish
set. Goal: `npx -y @solvela/mcp-server` resolves and runs. Both packages
sign/broadcast real USDC-SPL transactions on Solana mainnet, so this is a
money-path release — follow the gates in order, halt on the first failure.

Workflows:
- `.github/workflows/sdk-signer-core-publish.yml` — Layer 1.
- `.github/workflows/sdk-mcp-publish.yml` — Layer 2 (depends on Layer 1).

## Dependency order

```
@solvela/mcp-server          (Layer 2, sdks/mcp/)
        │  depends on
        │    "@solvela/signer-core": "file:../signer-core"   (in-repo, local dev)
        │    -> rewritten in CI to "^<version>"              (in the published tarball)
        ▼
@solvela/signer-core         (Layer 1, sdks/signer-core/)
        │  depends on
        ▼
@solana/web3.js, @solana/spl-token, valibot, bs58 …   (already on npm)
```

Publish strictly bottom-up: **signer-core first, then mcp-server.** The
mcp workflow will not publish until signer-core@<version> is resolvable on
npm (it polls and fails closed otherwise).

## Pre-fire gates (operator checklist — stop on first failure)

### Gate 0 — `repository` field on BOTH manifests (provenance precondition)

`npm publish --provenance` **requires** a `repository` field in
`package.json` that matches `github.com/solvela-ai/solvela`, and
`npm publish --dry-run` does **not** catch its absence — the failure only
appears on the real publish. As of 2026-06-29 **neither** manifest has it:

- `sdks/signer-core/package.json` — no `repository` field.
- `sdks/mcp/package.json` (the PR #646 publish-ready version) — no
  `repository` field.

Add to each (the `directory` differs per package):

```jsonc
"repository": {
  "type": "git",
  "url": "git+https://github.com/solvela-ai/solvela.git",
  "directory": "sdks/signer-core"   // or "sdks/mcp"
}
```

Fold the mcp change into **PR #646** (it owns the publish-ready mcp
manifest); add the signer-core change via a small follow-up commit on
main. Both publish workflows' `verify` job enforces this field and fails
closed with the exact snippet if it is missing — so a forgotten field
halts the run in seconds, it never publishes-without-provenance.

Source: https://docs.npmjs.com/generating-provenance-statements
Evidence: `@solvela/sdk` (has the field) publishes with provenance fine;
`@solvela/ai-sdk-provider` (lacks it) has never published.

### Gate 1 — PR #646 merged to main

The publish-ready `sdks/mcp/package.json` (drops `private`, adds the
`files` allowlist + `prepublishOnly`) lives on PR #646
(`feat/publish-mcp-web-search`). It **must** be merged to `main` before
tagging mcp. The mcp `verify` job hard-fails if it sees `"private": true`,
so a too-early tag halts safely — but don't rely on that; merge #646
first. (signer-core's publish-ready manifest is already on main.)

### Gate 2 — environment + secret

- Environment `npm-publish` exists on the repo (confirmed 2026-06-29).
  The publish jobs run in it; any required-reviewer / branch protection on
  that environment will gate the publish (desirable for a money path).
- `NPM_TOKEN` is a **repo-level** secret (confirmed present, last rotated
  2026-06-06). It is the same secret the existing npm publish workflows
  (`sdk-typescript-publish.yml`, `ai-sdk-provider-publish.yml`) use, and
  `@solvela/sdk@0.3.0` published with it recently — so it is known-good.
  Confirm it is an **automation/granular token** with publish rights to
  the `@solvela/*` scope and not expired. There is no token fallback:
  a missing/expired token fails the publish closed (correct behavior).

### Gate 3 — versions correct and unpublished

- Both first versions are `0.1.0` (matches both manifests).
- Both are unpublished on npm as of 2026-06-29 (`npm view` → 404 for
  `@solvela/signer-core` and `@solvela/mcp-server`).
- The `verify` job re-checks tag==version and not-already-published at fire
  time; if either version has since been published, it halts.

## Firing sequence (maintainer pushes the release tags, in dep-order)

Tag the **same** post-#646-merge commit on `main` for both packages. Tags
are immutable — never force-move, retag, or delete a published tag.

```bash
# 1. Layer 1 — signer-core. Trigger sdk-signer-core-publish.yml.
git tag sdks/signer-core/v0.1.0 <main-commit-sha>
git push origin sdks/signer-core/v0.1.0

# 2. Wait for the "sdk-signer-core publish" run to go fully green.
#    It ends with a registry SHA-match: signer-core@0.1.0 is now live.
#    (Do NOT push the mcp tag until this is green — the mcp workflow
#     polls for signer-core@0.1.0 and fails closed if it isn't there.)

# 3. Layer 2 — mcp-server. Trigger sdk-mcp-publish.yml.
git tag sdks/mcp/v0.1.0 <same-main-commit-sha>
git push origin sdks/mcp/v0.1.0

# 4. Wait for the "sdk-mcp publish" run to go fully green. Its final step
#    runs `npx -y @solvela/mcp-server@0.1.0` (pulling signer-core from the
#    registry) and asserts an MCP `initialize` response — i.e. it proves
#    the end goal: `npx -y @solvela/mcp-server` resolves.
```

What each workflow enforces, in order (so you can trust the gates):
`verify` (tag==version, not-private, repository present, not-already-
published) → `test` (full suite at the tagged commit, Node 22) → `publish`
(dry-run tarball check → [mcp only] swap `file:`→`^<version>` → [mcp only]
poll signer-core resolvable → `npm publish --access public --provenance` →
registry SHA-256 match → [mcp only] npx smoke).

## Rollback plan (partial / failed publish)

npm publishes are **not** transactionally reversible and **yanking is a
human decision** — this runbook never yanks.

- **signer-core published, mcp failed before publishing:** signer-core@0.1.0
  is live and harmless on its own (it is a library dep). Fix the mcp issue,
  then re-run the **failed mcp workflow run** from the Actions UI (idempotent:
  `verify` re-passes, publish proceeds). Only if the tag itself was wrong
  (wrong commit/version) cut a **new** tag at a new version — do not delete
  or move `sdks/mcp/v0.1.0`.
- **mcp published, then SHA-match or npx smoke failed:** mcp@0.1.0 IS live.
  Investigate immediately — a SHA mismatch or smoke failure on a money-path
  package is release-blocking. Whether to **yank** `@solvela/mcp-server@0.1.0`
  (or `@solvela/signer-core@0.1.0`) is an **operator decision**; this agent
  will not run `npm unpublish`/yank. If the bytes are bad, the operator
  yanks and a fixed `0.1.1` is cut.
- **Re-running against the same tag:** safe. `verify`'s already-published
  check makes a second live publish of the same version fail closed rather
  than corrupt state.
- Never: `npm unpublish`, yank, force-move/retag/delete a tag, or
  `--no-verify`.

## Test plan / rehearsal

- **Local dry-runs (done 2026-06-29, Node 22.22.0 + npm 10.9.4):**
  - signer-core: `npm ci` + `npm run build` + `npm test` (125 pass);
    `npm publish --dry-run` → 38 files / 37.6 kB, `dist/` + `README.md` +
    `package.json` only.
  - mcp (against the #646 manifest): `npm ci` + `npm run build` +
    `npm test` (112 pass); jq swap `file:../signer-core` → `^0.1.0`;
    `npm publish --dry-run` → 34 files / 54.4 kB, `dist/` + `README.md` +
    `package.json` only; packed `package.json` has `private` removed, the
    `bin` `dist/index.js` present (shebang preserved), and the swapped
    `^0.1.0` dep. No `src`/`tests`/`AGENTS.md`/secrets in either tarball.
- **CI plumbing rehearsal (optional, before the real tags):** push an rc
  tag pair (`sdks/signer-core/v0.1.0-rc.1`, then `sdks/mcp/v0.1.0-rc.1`).
  The workflows publish under the `rc` dist-tag — note this is a **real**
  publish to the prerelease channel, not a dry run. To exercise CI without
  any real publish, run the workflows on a **fork** where `NPM_TOKEN` is
  absent: `verify`/`test`/dry-run pass and the live publish fails closed.

## Cross-references

- Workflows authored here:
  `.github/workflows/sdk-signer-core-publish.yml`,
  `.github/workflows/sdk-mcp-publish.yml`.
- Mirrored reference workflows: `sdk-typescript-publish.yml`,
  `ai-sdk-provider-publish.yml` (npm auth/provenance/SHA-verify),
  `sdk-python-publish.yml`, `sdk-rust-publish.yml` (verify-first multi-job
  shape, propagation polls).
- SDK CI mirrored for the `test` jobs: `.github/workflows/mcp.yml`
  (Node 22, signer-core `file:` dep build dance).
- PR #646 (`feat/publish-mcp-web-search`) — makes the mcp manifest
  publish-ready; must merge before tagging mcp.
- Related: `docs/runbooks/sdk-consolidation.md`.
