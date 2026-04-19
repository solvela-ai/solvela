# Release Runbook — @solvela/ai-sdk-provider

This file documents how to cut, promote, and roll back releases of
`@solvela/ai-sdk-provider`. The publish workflow is CI-only — no
manual `npm publish` from a developer laptop.

---

## Prerequisites (one-time setup)

- `@solvela` npm org exists and you have publish rights.
- `NPM_TOKEN` secret is set in GitHub Actions (automation token, 2FA required,
  scoped to `@solvela/ai-sdk-provider`).
- A GPG key is configured on your GitHub account for signed tags (`git tag -s`).
- The `npm-publish` GitHub Environment is created in repo Settings →
  Environments. Add required reviewers there to gate the publish job behind a
  human approval step.

---

## Cutting a release

### 1. Verify the build is green

All CI checks on `main` must be passing before you tag.

### 2. Bump the version

Edit `sdks/ai-sdk-provider/package.json` — set `"version"` to the new value
(e.g. `"0.1.0"` or `"0.1.0-rc.1"`). Commit and merge to `main`.

### 3. Create and push a signed tag

The publish workflow triggers ONLY on tags matching `sdks/ai-sdk-provider/v*`.
Use the exact format below — any deviation will not trigger CI.

```bash
# Full release (publishes to `latest` dist-tag)
git tag -s sdks/ai-sdk-provider/v0.1.0 -m "ai-sdk-provider v0.1.0"
git push origin sdks/ai-sdk-provider/v0.1.0

# Release candidate (publishes to `rc` dist-tag)
git tag -s sdks/ai-sdk-provider/v0.1.0-rc.1 -m "ai-sdk-provider v0.1.0-rc.1"
git push origin sdks/ai-sdk-provider/v0.1.0-rc.1
```

The `-s` flag creates a GPG-signed tag. If your signing key is not configured,
`git tag -s` will fail — do not fall back to `git tag -a` for production
releases.

### 4. Approve the environment gate

GitHub will pause the `publish` job for reviewer approval because the job runs
inside the `npm-publish` environment. A required reviewer must approve in the
GitHub Actions UI before `npm publish` executes.

### 5. Verify provenance

After the workflow completes:

```bash
npm view @solvela/ai-sdk-provider@0.1.0
# Look for: "dist.provenance" or the provenance badge on npmjs.com
```

---

## Promoting an RC to `latest`

When an RC has been validated and is ready to ship as the stable release,
promote it without republishing:

```bash
npm dist-tag add @solvela/ai-sdk-provider@0.1.0-rc.1 latest
```

This reassigns the `latest` dist-tag to the existing RC tarball — no new
tarball is published, provenance is preserved.

---

## Deprecating a broken version

If a broken release reaches npm, deprecate it immediately so package managers
surface a warning to consumers:

```bash
npm deprecate @solvela/ai-sdk-provider@0.1.0 "broken release — use 0.1.1 instead"
```

Then publish a fix as `0.1.1` via the normal tag workflow.

---

## NEVER use `npm unpublish`

Do NOT run `npm unpublish` on any published version. Reasons:

1. **Retention policy** — npm's 72-hour unpublish window exists for new packages
   only; after that, unpublish is permanently blocked for scoped packages with
   dependents.
2. **Consumer disruption** — any consumer pinned to the unpublished version gets
   a broken install with no fallback. `npm deprecate` achieves the warning
   without breaking existing installs.
3. **Provenance chain break** — unpublishing severs the OIDC provenance
   attestation. The SHA on the npm transparency log is permanent; the tarball
   disappearing behind it creates an integrity gap.

If a version contains a critical security vulnerability, use `npm deprecate`
with a clear message and publish a patched version immediately. Contact npm
support only if the situation involves exposed secrets that require emergency
removal.

---

## Rollback procedure

If `0.1.0` ships broken:

1. Deprecate: `npm deprecate @solvela/ai-sdk-provider@0.1.0 "broken release, use 0.1.1"`
2. Cut a patch: bump version to `0.1.1`, fix the issue, push a new signed tag
   `sdks/ai-sdk-provider/v0.1.1`.
3. Verify `npm view @solvela/ai-sdk-provider` shows `0.1.1` as `latest`.

---

## Version policy summary

| Tag format | npm dist-tag | Example |
|---|---|---|
| `sdks/ai-sdk-provider/vX.Y.Z` | `latest` | `v0.1.0` |
| `sdks/ai-sdk-provider/vX.Y.Z-rc.N` | `rc` | `v0.1.0-rc.1` |

Bump to `v0.2.0` when upgrading the target to `LanguageModelV4` / `ai@^7.0.0`
(see plan §3.1 upgrade path). That bump is a peer-dep range change and a
single-line `specificationVersion` flip — no architectural rework required.
