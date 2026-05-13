# Release Checklist

A practical "before you ship" playbook for `solvela-sdk`. Run top-to-bottom for any release. Each section says **what to run** and **what it catches** so you know what risk remains if you skip a step.

---

## Mental model (for context, you don't need to memorize)

Tests in this repo split into three tiers, each catching a different class of bug:

| Tier | Path | Speed | What it catches | What it can't catch |
|---|---|---|---|---|
| **Unit** | `tests/unit/` | seconds | Pure-Python logic: parsing, validation, type errors | Anything that talks to a network |
| **Integration** | `tests/integration/` | seconds | HTTP shape, header names, JSON encoding (via mocked transport) | Whether the real gateway agrees with your fixture |
| **Live** | `tests/live/` | depends | The actual wire contract against a running gateway | Performance under load, real-user concurrency |

CI (GitHub Actions) runs Unit + Integration on every push for Python 3.10, 3.11, 3.12. Live tests are gated by `SOLVELA_LIVE_TESTS=1` and never run in CI — you run them yourself.

---

## Cycle 1 — Local verification (every commit, ~30 seconds)

Run from repo root with the venv active (`.venv/bin/activate` or use `.venv/bin/python` directly).

```bash
.venv/bin/python -m pytest tests/unit/ tests/integration/ -q
.venv/bin/python -m ruff check src/ tests/
.venv/bin/python -m ruff format --check src/ tests/
.venv/bin/python -m mypy src/
```

**What each one catches:**

- `pytest` — your code's behavior matches your tests. Run before every push.
- `ruff check` — common Python mistakes (unused imports, undefined names, bad style). Fast static analysis. Catches roughly 80% of "looks weird" issues.
- `ruff format --check` — formatting consistency. Doesn't catch bugs, prevents bikeshedding.
- `mypy src/` — type errors. Currently has 8 known errors (pre-existing baseline). If the count goes UP, you introduced a type bug. If it goes down, even better.

**Pass criteria:** all tests green, ruff clean, mypy at the same baseline (8 errors, none in code you just touched).

**Doesn't catch:** anything network, real wallet keys, gateway-side disagreement.

---

## Cycle 2 — Open a PR

```bash
git checkout -b fix/your-thing
# ... make changes, then ...
git add <specific files>
git commit -m "concise summary of what changed and why"
git push -u origin fix/your-thing
gh pr create --title "..." --body "..."
```

Once the PR exists, CI runs automatically. Watch for green check marks on the PR page or via:

```bash
gh pr checks <PR-number>
```

**What CI catches that local doesn't:**
- Differences between Python 3.10, 3.11, 3.12 (you have one local Python; CI has all three)
- Forgotten files (`git add` missed something) — CI installs from a fresh clone, so anything not committed breaks the build

**Doesn't catch:** anything live or wallet-related.

---

## Cycle 3 — Live tests (before tagging a release)

This is the step that catches "looked clean → broke in prod."

### What you need

- A running Solvela gateway (Rust/Axum). The default URL is `http://localhost:8402`. Override with `SOLVELA_GATEWAY_URL=https://your-gateway.example.com`.
- For tests that need payment: a funded test wallet. Today's live tests (`tests/live/test_live_chat.py`) only check `models()` and the unsigned 402 path, so no wallet is needed for the current suite.

### Run them

```bash
SOLVELA_LIVE_TESTS=1 .venv/bin/python -m pytest tests/live/ -v
# or, with a custom gateway:
SOLVELA_LIVE_TESTS=1 SOLVELA_GATEWAY_URL=https://staging.solvela.ai .venv/bin/python -m pytest tests/live/ -v
```

**What this catches:**
- Wire format drift: your `from_dict` / `to_dict` round-trips no longer match what the gateway sends
- Header name typos (`Payment-Signature` vs `payment-signature`)
- The gateway adding new required fields you don't model
- `accepts` array ordering, error message phrasing, scheme names

**Doesn't catch:**
- Performance under load (use a load test for that)
- Concurrency races (use `pytest-asyncio` stress + threaded scenarios)
- Bugs in code paths not exercised by these tests

### If you don't have a gateway running

The next-best thing is a **manual smoke test** — a 10-line script that hits the gateway with a real request. See **Cycle 4** below. You can skip live tests, but you should never skip the smoke test on a release.

---

## Cycle 4 — Manual smoke test (mandatory before tagging)

Even with all of the above green, write a tiny script that does the actual end-to-end thing your users will do, then run it manually before tagging a release. Save it as `scripts/smoke.py`:

```python
"""Manual smoke test — exercises the real wire contract before release.

Usage:
    SOLVELA_GATEWAY_URL=https://staging.solvela.ai \
    .venv/bin/python scripts/smoke.py
"""
import asyncio
import os

from solvela.client import SolvelaClient
from solvela.config import ClientConfig
from solvela.errors import PaymentRequiredError
from solvela.types import ChatMessage, ChatRequest, Role


async def main() -> None:
    gateway_url = os.environ.get("SOLVELA_GATEWAY_URL", "http://localhost:8402")
    print(f"Smoke test against: {gateway_url}")

    client = SolvelaClient(config=ClientConfig(gateway_url=gateway_url))

    # 1. Models endpoint reachable.
    models = await client.models()
    print(f"  models()         -> {len(models)} model(s) returned")
    assert len(models) > 0, "expected at least one model"

    # 2. Unsigned chat returns 402 (payment required).
    req = ChatRequest(
        model=models[0].id,
        messages=[ChatMessage(role=Role.USER, content="ping")],
    )
    try:
        await client.chat(req)
        raise SystemExit("FAIL: expected PaymentRequiredError, got a response")
    except PaymentRequiredError as exc:
        total = exc.payment_required.cost_breakdown.total
        currency = exc.payment_required.cost_breakdown.currency
        print(f"  chat() unsigned  -> 402 OK ({total} {currency})")

    print("\nSmoke test PASSED")


if __name__ == "__main__":
    asyncio.run(main())
```

**Run it:**

```bash
SOLVELA_GATEWAY_URL=https://your-staging-gateway .venv/bin/python scripts/smoke.py
```

**Why this matters more than another code review:** every assertion here is a fact about the actual world. If the gateway changed its 402 body shape, this dies. If the cost breakdown field renamed, this dies. If the `models()` endpoint moved, this dies. None of those would show up in unit tests, integration tests, or code review.

**Tell me if you want this script created** — I can write it and add a paid-flow variant that signs a transaction with a devnet wallet (a more thorough check, but it costs USDC).

---

## Cycle 5 — Tag a release

The release pipeline (`.github/workflows/release-pypi.yml`) fires when you push a `v*` tag. It:

1. Verifies the git tag matches `pyproject.toml`'s `version` (refuses to publish if they disagree).
2. Builds the wheel + sdist.
3. Publishes to PyPI via OIDC Trusted Publishing (no API token needed).
4. Emits PEP 740 attestations (cryptographic proof the release came from this repo).
5. Creates a GitHub Release.

### Steps

```bash
# 1. Bump the version in pyproject.toml. Use semver:
#    - 0.1.3 -> 0.1.4 for bug fixes
#    - 0.1.4 -> 0.2.0 for new features (breaking is OK pre-1.0)
#    - 0.x   -> 1.0.0 when you're ready to commit to API stability
$EDITOR pyproject.toml   # change `version = "0.1.3"` to whatever

# 2. Commit the bump on its own.
git add pyproject.toml
git commit -m "chore(release): bump to 0.1.4"
git push origin main

# 3. Tag it (annotated, with a message).
git tag -a v0.1.4 -m "Release 0.1.4: review fixes for #7, #8, #11, #12"
git push origin v0.1.4

# 4. Watch the release workflow.
gh run watch
# or
gh run list --workflow=release-pypi.yml --limit 1
```

**Pass criteria:** the workflow finishes green, and the package shows up on https://pypi.org/project/solvela-sdk/.

**If it fails:** read the workflow log (`gh run view --log-failed`). Common causes:
- Tag doesn't match `pyproject.toml` version (the workflow blocks this — fix one or the other).
- PyPI Trusted Publisher not configured (one-time setup; check repo settings).
- Tests fail on a different Python version than yours.

### Pre-release safety: TestPyPI dry run (recommended for major changes)

PyPI has a sister index, `test.pypi.org`, that you can publish to first. For `0.x` SDKs the convention is:

1. Tag with a `-rc1` suffix: `v0.1.4-rc1`.
2. Manually invoke a test build — or just upload to TestPyPI by hand:
   ```bash
   .venv/bin/python -m pip install build twine
   .venv/bin/python -m build
   .venv/bin/python -m twine upload --repository testpypi dist/*
   ```
3. Install from TestPyPI in a fresh venv:
   ```bash
   python -m venv /tmp/test-install
   /tmp/test-install/bin/pip install --index-url https://test.pypi.org/simple/ \
       --extra-index-url https://pypi.org/simple/ \
       solvela-sdk==0.1.4rc1
   ```
4. Run your smoke test against the freshly-installed copy.
5. Only after that passes, tag the real release.

**Tell me if you want me to wire up an automated TestPyPI workflow** — about 30 lines of YAML.

---

## Cycle 6 — After release

For a week after a release, watch:

1. **GitHub Issues**: `gh issue list --state open --limit 20`. Real users open these. Triage daily.
2. **PyPI download stats**: https://pypistats.org/packages/solvela-sdk — confirms people are actually picking up the new version.
3. **GitHub Actions failure rate**: `gh run list --limit 20`. If CI starts failing on `main` after a release, something's wrong.
4. **If you have telemetry on the gateway side**: error rates from clients identifying as the new version. (You'd have to add a `User-Agent: solvela-sdk/0.1.4` header in `transport.py` for this — currently not done. Tell me if you want it.)

**The trauma scenario you're worried about** — "it looked clean but broke in prod" — is almost always caught between Cycle 4 (manual smoke) and Cycle 6 (post-release watch). Code review catches design problems; tests catch regressions; live tests catch wire-format drift; smoke tests catch "did I actually wire it together"; post-release monitoring catches everything else.

---

## What I would do for THIS release

You just merged 4 PRs (#7, #8, #11, #12). My honest recommendation in priority order:

1. **Cycle 1** locally on `main`. (~30 sec) — sanity check the merged result.
2. **Cycle 4** — manual smoke test against any reachable gateway, even a local docker-compose dev gateway. (~1 min)
3. If a staging gateway exists, **Cycle 3** live tests. (~30 sec)
4. **Cycle 5** — bump to `0.1.4`, tag, push. The post-release Cycle 6 watching is automatic if you check email for GitHub notifications.

If no gateway is reachable at all, you can ship to PyPI as `0.1.4-rc1` (release candidate), let yourself or a friend pip-install it, run the smoke test, and only then promote to `0.1.4`. That's the lowest-risk path that doesn't require infrastructure.

---

## Glossary (for context — skip if you know these)

- **venv** — a self-contained Python environment per project. `.venv/bin/python` is "this project's Python." Equivalent in spirit to AS3's project-level SDK selection — keeps libraries from one project from breaking another.
- **pip install -e .** — installs the package in editable mode. Code edits in `src/` show up immediately in the venv without reinstalling.
- **pytest** — the test runner. Roughly equivalent to AS3's FlexUnit / asunit but smarter; auto-discovers `test_*.py` files.
- **ruff** — combined linter + formatter. Replaces flake8/pylint/black/isort. Fast (Rust-based).
- **mypy** — static type checker. Reads the type annotations in your code and flags inconsistencies. AS3 had types; Python types are *optional* and only enforced when you opt in.
- **CI** — Continuous Integration. GitHub Actions runs your tests on every push automatically. The `.github/workflows/*.yml` files describe what it runs.
- **PyPI** — the Python Package Index. `pip install solvela-sdk` pulls from here. Equivalent to npm for JavaScript or the AS3 component marketplace.
- **OIDC Trusted Publishing** — modern way to publish to PyPI without storing an API token in GitHub. The PyPI side trusts that GitHub Actions running on this specific repo is allowed to publish; auth is short-lived and minted per-build.
- **Semver** (semantic versioning) — `MAJOR.MINOR.PATCH`. Bump PATCH for fixes, MINOR for backward-compatible features, MAJOR for breaking changes. Pre-1.0 (`0.x`) all bets are off; that's where this project is.
- **Force-push (`--force-with-lease`)** — overwrites a remote branch with a different history. Used for rebasing feature branches. `--with-lease` adds a safety check that aborts if someone else pushed first. Never force-push to `main`.

---

## Common commands cheat sheet

```bash
# What's the current state of things?
git status                          # local working tree
git log --oneline -5                # last 5 commits
gh pr list --state open             # open PRs
gh pr checks <number>               # CI for a specific PR
gh run list --limit 5               # last 5 workflow runs

# Run the suite locally
.venv/bin/python -m pytest tests/unit/ tests/integration/ -q

# Live tests against a gateway
SOLVELA_LIVE_TESTS=1 .venv/bin/python -m pytest tests/live/ -v

# Lint + format + types
.venv/bin/python -m ruff check src/ tests/
.venv/bin/python -m ruff format --check src/ tests/
.venv/bin/python -m mypy src/

# Release dance
$EDITOR pyproject.toml              # bump version
git commit -am "chore(release): bump to X.Y.Z"
git push
git tag -a vX.Y.Z -m "..."
git push origin vX.Y.Z
gh run watch                        # watch the publish job
```
