# Runbook: SDK consolidation into the monorepo

> Status as of 2026-05-12: Go SDK done ([#252](https://github.com/solvela-ai/solvela/pull/252), [#257](https://github.com/solvela-ai/solvela/pull/257)). TypeScript, Python, and Rust client remaining.

## Why this exists

On 2026-05-11 four separate SDKs (`solvela-ts`, `solvela-python`, `solvela-go`, `solvela-client`) shipped wire-format fixes within hours of each other because nothing was asserting the gateway's `ModelInfo` / 402 shape across the repo boundary. The structural fix is to pull each SDK into the gateway monorepo at `sdks/<lang>/`, so a change to `crates/protocol/**` that breaks any SDK fails CI in the same PR that introduces it.

Each consolidation is one PR, independently shippable. Stopping after any one is a clean end-state — there is no half-completed multi-PR sequence to clean up.

## Order

1. **TypeScript** (next) — `sdks/signer-core/` already proves the in-monorepo TS pattern. Lowest risk.
2. **Python** — mostly mechanical; clean up stale `rustyclawrouter/` cruft in `sdks/python/` while moving.
3. **Rust client** — different shape (non-workspace member). Defer until you actually want it; the Solana-dep conflict makes it the highest-effort one.

## The pattern

For each SDK:

1. Branch off `main` in the gateway repo (`solvela-ai/solvela`).
2. `rsync` source from the external repo into `sdks/<lang>/`, excluding `.git/`, `.github/`, `.gitignore`, `.mcp.json`, `LICENSE`, `SECURITY.md` (monorepo-root files apply), build artifacts (`node_modules/`, `dist/`, `target/`, `.venv/`).
3. Rewrite any package-manifest fields that reference the old standalone repo (`repository.url`, `homepage`, `bugs.url`, Go module path, etc.). The published package name (`@solvela/sdk`, `solvela-sdk`, etc.) stays the same so installs are unchanged.
4. Verify the SDK still builds and tests pass locally at the new path.
5. Add `.github/workflows/sdks-<lang>.yml` — path-scoped on `sdks/<lang>/**`, mirroring the [`mcp.yml`](../../.github/workflows/mcp.yml) pattern.
6. One commit, push, open PR. Bot (`solvela-per`) approves to satisfy branch protection, then squash-merge.
7. **Post-merge:** push a path-prefixed version tag (`sdks/<lang>/v<semver>`) so the language's package proxy discovers the new path. Replace the external repo's README with a 4-line redirect pointing at the monorepo. `gh repo archive <external-repo> --yes`.
8. **Doc follow-up PR:** prepend a new dated section to [`CHANGELOG.md`](../../CHANGELOG.md) and update the SDKs bullet in [`STATUS.md`](../../STATUS.md).

The shape is verified — PR [#252](https://github.com/solvela-ai/solvela/pull/252) (Go consolidation) and PR [#257](https://github.com/solvela-ai/solvela/pull/257) (doc update) are the reference implementation.

---

## PR #2 — TypeScript

**Pre-flight check at start of session:**

```bash
cd /home/kennethdixon/projects/solvela-ts && git status && git log -1 --oneline
ls /home/kennethdixon/projects/solvela/sdks/typescript/
```

Expected: clean tree, latest commit `fb46b40` (smoke drift assertions); monorepo `sdks/typescript/` contains only `dist/` + `node_modules/` artifacts from the d2824e0a refactor, no source.

**Branch + copy:**

```bash
cd /home/kennethdixon/projects/solvela
git checkout main && git pull --ff-only
git checkout -b consolidate/sdk-typescript

SRC=/home/kennethdixon/projects/solvela-ts
DST=sdks/typescript
rm -rf "$DST"
mkdir -p "$DST"
rsync -av \
  --exclude='.git' --exclude='.github' --exclude='.gitignore' \
  --exclude='.mcp.json' --exclude='LICENSE' --exclude='SECURITY.md' \
  --exclude='node_modules' --exclude='dist' \
  --exclude='.venv' --exclude='.pytest_cache' \
  --exclude='.mypy_cache' --exclude='.ruff_cache' \
  "$SRC/" "$DST/"
```

**Rewrite package.json metadata (keep `name = "@solvela/sdk"`):**

```bash
node -e "
const fs = require('fs');
const p = JSON.parse(fs.readFileSync('$DST/package.json', 'utf8'));
p.repository = { type: 'git', url: 'git+https://github.com/solvela-ai/solvela.git', directory: 'sdks/typescript' };
p.homepage = 'https://github.com/solvela-ai/solvela/tree/main/sdks/typescript#readme';
p.bugs = { url: 'https://github.com/solvela-ai/solvela/issues' };
fs.writeFileSync('$DST/package.json', JSON.stringify(p, null, 2) + '\n');
"
```

**Verify locally:**

```bash
( cd "$DST" && npm ci && npm run build && npx vitest run tests/unit tests/integration )
```

**Workflow** — create `.github/workflows/sdks-typescript.yml`:

```yaml
name: sdks-typescript
on:
  push:
    paths: ['sdks/typescript/**', '.github/workflows/sdks-typescript.yml']
  pull_request:
    paths: ['sdks/typescript/**', '.github/workflows/sdks-typescript.yml']
defaults:
  run:
    working-directory: sdks/typescript
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        node: ['20', '22']
    steps:
      - uses: actions/checkout@v6
      - uses: actions/setup-node@v6
        with:
          node-version: ${{ matrix.node }}
          cache: npm
          cache-dependency-path: sdks/typescript/package-lock.json
      - run: npm ci
      - run: npx tsc --noEmit
      - run: npx vitest run tests/unit tests/integration
```

**Commit + PR:**

```bash
git add sdks/typescript/ .github/workflows/sdks-typescript.yml
git commit -m "feat(sdks/typescript): pull TypeScript SDK into the monorepo"
git push -u origin consolidate/sdk-typescript
gh pr create --title "feat(sdks/typescript): pull TypeScript SDK into the monorepo" \
  --body "Pulls @solvela/sdk source from solvela-ai/solvela-ts into sdks/typescript/. Package name @solvela/sdk preserved so npm installs are unchanged. Mirrors the PR #252 pattern for Go."
```

**Watch CI, bot-approve, merge:**

```bash
gh pr checks <PR#> --watch --interval 30
gh auth switch -u solvela-per
gh pr review <PR#> --approve --body "CI green. Mirrors the Go consolidation. Approved."
gh auth switch -u sky64
gh pr merge <PR#> --squash --delete-branch
```

**Post-merge — tag + archive:**

```bash
git checkout main && git pull --ff-only
git tag -a sdks/typescript/v0.2.1 -m "TypeScript SDK initial release after monorepo consolidation"
git push origin sdks/typescript/v0.2.1

cd /home/kennethdixon/projects/solvela-ts
# Hand-edit README.md to a short redirect:
#   # solvela-ts
#   > **This SDK has moved.** Source now lives at:
#   > https://github.com/solvela-ai/solvela/tree/main/sdks/typescript
#   `npm install @solvela/sdk` is unchanged.
git add README.md
git commit -m "docs: redirect to monorepo at solvela-ai/solvela/sdks/typescript

Signed-off-by: sky4 <kd@sky64.io>"
git push origin main
gh repo archive solvela-ai/solvela-ts --yes
```

---

## PR #3 — Python

**Pre-flight check:**

```bash
cd /home/kennethdixon/projects/solvela-python && git status && git log -1 --oneline
ls /home/kennethdixon/projects/solvela/sdks/python/
```

Expected: clean tree, latest commit `200e869` (smoke fixture asserts); monorepo `sdks/python/` still contains stale `rustyclawrouter/` + `tests/` + `README.md` from before the d2824e0a refactor. The `rm -rf` below cleans that.

**Branch + copy:**

```bash
cd /home/kennethdixon/projects/solvela
git checkout main && git pull --ff-only
git checkout -b consolidate/sdk-python

SRC=/home/kennethdixon/projects/solvela-python
DST=sdks/python
rm -rf "$DST"
mkdir -p "$DST"
rsync -av \
  --exclude='.git' --exclude='.github' --exclude='.gitignore' \
  --exclude='.mcp.json' --exclude='LICENSE' --exclude='SECURITY.md' \
  --exclude='.venv' --exclude='.pytest_cache' \
  --exclude='.mypy_cache' --exclude='.ruff_cache' \
  "$SRC/" "$DST/"
```

Package name `solvela-sdk` already matches PyPI — no rewrite needed beyond optional `repository`/`homepage` URLs in `pyproject.toml`.

**Verify:**

```bash
( cd "$DST" && python -m pip install -e ".[dev]" && pytest tests/ && ruff check . && ruff format --check . )
```

**Workflow** — `.github/workflows/sdks-python.yml`:

```yaml
name: sdks-python
on:
  push:
    paths: ['sdks/python/**', '.github/workflows/sdks-python.yml']
  pull_request:
    paths: ['sdks/python/**', '.github/workflows/sdks-python.yml']
defaults:
  run:
    working-directory: sdks/python
jobs:
  test:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        python: ['3.10', '3.12']
    steps:
      - uses: actions/checkout@v6
      - uses: actions/setup-python@v6
        with:
          python-version: ${{ matrix.python }}
          cache: pip
      - run: pip install -e ".[dev]"
      - run: ruff check .
      - run: ruff format --check .
      - run: pytest tests/ -v
```

**Same commit / PR / bot-approve / squash-merge / post-merge tag-and-archive sequence as PR #2.** Tag: `sdks/python/v0.2.0`. Archive: `gh repo archive solvela-ai/solvela-python --yes`.

---

## PR #4 — Rust client (non-workspace member)

This one is **different** from the others. `solvela-client` is its own Cargo workspace with 4 crates (`solvela-client`, `-cli`, `-cli-args`, `-proxy`), uses `solana-sdk = "~4.0"` and `spl-token = "9"`, and depends on `solvela-protocol = "0.1"` from crates.io. The gateway workspace uses older Solana versions. Pulling it in as a workspace member forces dep resolution conflicts.

**Solution: drop it at `sdks/rust/` as a non-workspace member, mirroring `programs/escrow/`.** Top-level `Cargo.toml` adds `sdks/rust` to its `exclude` list. Cargo treats it as a separate build graph. No version conflict.

**Pre-flight checks (do these first — if either trips, stop and decide):**

```bash
# Confirm the workspace's solvela-protocol version equals (or is forward-compatible with)
# the published crate the Rust client depends on:
grep '^version' /home/kennethdixon/projects/solvela/crates/protocol/Cargo.toml
grep 'solvela-protocol' /home/kennethdixon/projects/solvela-client/Cargo.toml

# Confirm the exclude pattern exists on the top-level workspace Cargo.toml:
grep -A 5 'exclude' /home/kennethdixon/projects/solvela/Cargo.toml
```

If the workspace `solvela-protocol` version is ≥ the published one the Rust client depends on, you can later switch the SDK's dep to a workspace-relative `path = "../../crates/protocol"` to get compile-time drift detection. **Defer that decision** — keep the crates.io dep for the consolidation PR; consider the path-dep switch in a follow-up if and when versions align.

**Branch + copy:**

```bash
cd /home/kennethdixon/projects/solvela
git checkout main && git pull --ff-only
git checkout -b consolidate/sdk-rust

SRC=/home/kennethdixon/projects/solvela-client
DST=sdks/rust
rm -rf "$DST"
mkdir -p "$DST"
rsync -av \
  --exclude='.git' --exclude='.github' --exclude='.gitignore' \
  --exclude='.mcp.json' --exclude='LICENSE' --exclude='SECURITY.md' \
  --exclude='target' --exclude='.claude' \
  "$SRC/" "$DST/"
```

**Add `sdks/rust` to the top-level workspace exclude list:**

Edit `/home/kennethdixon/projects/solvela/Cargo.toml` — find the `[workspace]` `exclude` array (which already contains `"programs/escrow"`) and append `"sdks/rust"`. One line.

**Verify SDK builds in isolation AND workspace ignores it:**

```bash
# SDK on its own Cargo.lock + Solana 4 tree:
( cd sdks/rust && cargo check --all-targets )

# Workspace doesn't pick it up:
cargo check
cargo test --all -- --skip 'integration::live'
```

**Workflow** — `.github/workflows/sdks-rust.yml`:

```yaml
name: sdks-rust
on:
  push:
    paths: ['sdks/rust/**', '.github/workflows/sdks-rust.yml']
  pull_request:
    paths: ['sdks/rust/**', '.github/workflows/sdks-rust.yml']
defaults:
  run:
    working-directory: sdks/rust
jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@de0fac2e4500dabe0009e67214ff5f5447ce83dd  # v6.0.2
      - uses: dtolnay/rust-toolchain@631a55b12751854ce901bb631d5902ceb48146f7  # stable
        with:
          components: rustfmt, clippy
      - uses: Swatinem/rust-cache@c676846f29d98ff6b0106d3608c7ffd4048af17b  # v2
        with:
          workspaces: sdks/rust
          key: sdks-rust
      - run: cargo fmt --all -- --check
      - run: cargo clippy --all-targets -- -D warnings
      - run: cargo test --all
```

**Same commit / PR / bot-approve / squash-merge / post-merge tag-and-archive sequence.** Tag: `sdks/rust/v0.2.1`. Archive: `gh repo archive solvela-ai/solvela-client --yes`.

**One subtle follow-up specific to Rust:** `solvela-client` has its own `release-crates.yml` workflow that publishes to crates.io. The published crate name (`solvela-client`) doesn't change with the move, but the publishing workflow needs to be re-created in the monorepo (or just ported from the external repo's `.github/workflows/release-crates.yml`) before the next crates.io publish. Defer this until the consolidation PR merges — it's a separate decision.

---

## Per-PR doc follow-up

After each consolidation PR merges, ship a tiny doc PR like [#257](https://github.com/solvela-ai/solvela/pull/257):

```bash
cd /home/kennethdixon/projects/solvela
git checkout main && git pull --ff-only
git checkout -b docs/sdk-<lang>-consolidation
```

Then edit two files:

- **[`CHANGELOG.md`](../../CHANGELOG.md):** prepend a new dated section using the [#252](https://github.com/solvela-ai/solvela/pull/252) entry as a template. Keep it ~4 paragraphs: what moved, the tag, the archive, why it matters.
- **[`STATUS.md`](../../STATUS.md):** bump the PR-count in the "Last refreshed" line; add a fresh dated line above it noting this round; rewrite the Shipped/SDKs bullet so the just-consolidated SDK is listed under its monorepo path (`sdks/<lang>/v<semver>`) and removed from the "separate repos" list.

Same commit / bot-approve / merge dance as the consolidation PR.

After all four are done, the Shipped/SDKs bullet should read something like: *"All four SDKs (Python, TypeScript, Go, Rust client) live in the monorepo at `sdks/*/` and are tested per-PR; external single-SDK repos are archived with redirect READMEs."*

---

## Things to skip / known gotchas

- **Do not** add fixture-tarball / cross-repo contract tests / nightly drift watchers. That infrastructure is unnecessary once the SDKs are in the monorepo — the type checker is the drift detector.
- **Do not** try to merge `sdks/rust/` into `[workspace.members]`. Keep it as a non-workspace member like `programs/escrow/`. Same precedent in this repo, no Solana-dep conflict.
- **Bot-approval ceremony** is required for the main `solvela` repo because of branch protection. Use `gh auth switch -u solvela-per` → `gh pr review --approve` → `gh auth switch -u sky64` → `gh pr merge --squash --delete-branch`. About 30 seconds per PR.
- **External repos** (`solvela-ts`, `solvela-python`, `solvela-client`) — branch protection is not enforced for admins (verified for `solvela-go`; assume the others match, fall back to a tiny PR if any rejects). Direct push to main for the redirect-README commit is fine.
- **DCO signoff:** the gateway's `.githooks/prepare-commit-msg` auto-adds `Signed-off-by:`. The external repos don't have that hook — for the redirect-README commit, hand-add the trailer in the `-m` body.
- **`gh repo archive`** is one-way reversible (`gh repo unarchive`). Don't archive until the new tag is pushed and you've confirmed the new monorepo path resolves.
- **Stopping is fine.** If you do TS and not the others, that's still a structural win. There's no half-state to clean up.
- **The Rust client's crates.io publish pipeline** is a separate concern from consolidation. Don't conflate them.

## Cross-references

- Reference PR: [#252](https://github.com/solvela-ai/solvela/pull/252) — Go consolidation, the proven pattern.
- Reference doc PR: [#257](https://github.com/solvela-ai/solvela/pull/257) — the post-consolidation CHANGELOG/STATUS update.
- Cause-of-action: the 2026-05-12 entry in [`CHANGELOG.md`](../../CHANGELOG.md) covering the Go consolidation and the 2026-05-11 wire-format cascade that motivated it.
