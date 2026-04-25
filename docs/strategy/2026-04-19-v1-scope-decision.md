# V1.0 Scope Decision — PyPI and Go SDK

> **Question:** Should `solvela-sdk` (Python) and the Go SDK ship in V1.0 alongside the npm packages, or defer to V1.1?
>
> **Short answer:** Defer both. Ship npm-only in V1.0 (your runbook is right). Publish PyPI at **T+7 days**, Go at **T+14 days**. Specific rationale and tradeoffs below.

---

## 0 — Pre-work verified

| Name | Registry | Status |
|---|---|---|
| `solvela-sdk` | PyPI | ✅ AVAILABLE |
| `solvela` | PyPI | ✅ AVAILABLE |
| `solvela-python` | PyPI | ✅ AVAILABLE |
| `solvela-client` | PyPI | ✅ AVAILABLE |

PyPI naming is safe. No squatter problem like crates.io had for `x402`. This removes ambiguity from the decision — the *only* tradeoff is launch-day focus vs reach.

Go module path (`github.com/solveladev/solvela-go` per repo rename in HANDOFF) needs the repo to exist publicly at that path before any tag. Confirm.

---

## 1 — The case for including PyPI in V1.0

Real arguments, not faint-praise:

- **Python is ~30–40% of the AI agent developer universe.** Python agent frameworks: LangChain, LlamaIndex, AutoGPT, CrewAI, ElizaOS (partial), agents-sdk, etc. Missing them at launch hands that audience to competitors.
- **The SDK already exists** with 63 tests (per HANDOFF). Marginal work is packaging metadata polish + `twine upload`, not new engineering.
- **`pip install solvela-sdk`** is a natural commanding example for HN/Twitter copy. "Works in Python and TypeScript" is more credible than "works in TypeScript."
- **Competitive coverage:** BlockRun ships Python. Skyfire ships Python. Omitting Python reads as incomplete, even if it's tactical.
- **One audience, one day:** you can only "launch" once. Every channel you add on T+N lands with progressively less energy.

## 2 — The case for deferring PyPI to V1.1

- **Your V1.0 narrative is agent-native MCP.** The drafts lean hard into Claude Code / Cursor / Claude Desktop / OpenClaw — all JavaScript-host MCP consumers. Python isn't in that story. Shipping `solvela-sdk` on PyPI doesn't reinforce the MCP narrative; it dilutes focus to "here are all our SDKs."
- **Launch day is fragile.** Your Phase 4 runbook already has 9 `package.json` un-privates + 4 platform binaries + meta-package propagation ordering. Adding PyPI = one more surface area, one more registry dashboard to monitor, one more set of docs to audit, one more stream of user questions to field.
- **Python SDK readiness is unverified.** I haven't seen its README, build output, or the tagline. If it's 90% polished it's a day; if it's 70% polished, it's a week. Surprising yourself at launch time is bad.
- **Deferral buys a second attention spike.** Announcing "Python SDK now live" at T+7 is a legitimate refresh signal. Launching everything at T+0 collapses two moments into one.
- **Support burden:** Python users will hit PyPI and file issues before the dust settles on npm bugs. You're already going to be buried in X replies and GitHub issues for the first 48h.

## 3 — The case for including Go in V1.0

- **It's free.** Go publishing is just a git tag; no package build, no twine, no account setup.
- **"Works in 3 languages"** is stronger copy than "2."

## 4 — The case for deferring Go to V1.1

- **Go agent dev is <10% of the audience.** Most Go folks in AI land are writing infra, not agents.
- **Module path risk** (`github.com/solveladev/solvela-go` or whichever): if the path in `go.mod` doesn't match the actual public repo, `go get` fails silently and launch-day users think your SDK is broken.
- **No compelling launch narrative** where Go is the hero. "Agent in Go paying with USDC" isn't a story anyone's asking for in April 2026.

---

## 5 — Recommendation: staged release

| Phase | Timing | Ship |
|---|---|---|
| **V1.0** | Day 0 (OpenClaw smoke test passes) | npm suite + GitHub Release + MCP registry submissions + HN + X + blog |
| **V1.1-python** | T+7 days | PyPI (`solvela-sdk`) + minor blog post + single tweet |
| **V1.1-go** | T+14 days | Go tag + single tweet + docs updates |
| **V1.2** | T+30 days | crates.io decision (see separate doc) — publish `solvela-cli` to crates.io if adoption signals warrant |

### Why this shape

- **T+7 is a real attention window.** Your HN thread will be ~3 days dead by then; X momentum tapered; registry PRs hopefully merged. A "Python SDK now live" post lands into an audience that saw the original launch and is watching for movement. One more organic spike at zero marginal cost.
- **T+14 for Go** keeps the cadence. Two beats of "still shipping" after the main launch is the right amount of signal — more starts to look thirsty.
- **Focused V1.0 reduces failure modes.** Every extra channel = another thing that could break during your hardest 48 hours.

---

## 6 — The guardrails to set now (V1.0 prep, not launch)

Even though you're deferring, do these before V1.0 so the V1.1 steps are cheap:

### For PyPI (5 hours of work, now)
- Create PyPI + TestPyPI accounts; generate tokens; save to `~/.pypirc`
- Verify `solvela-sdk` builds clean: `cd sdks/python && python -m build && twine check dist/*`
- Test-upload to TestPyPI: `twine upload --repository testpypi dist/*`
- Install from TestPyPI in a fresh venv and run the README quickstart
- If all pass, the actual V1.1 step is a single `twine upload dist/*`

### For Go (1 hour of work, now)
- Confirm the module path in `sdks/go/go.mod` matches a real, public, readable GitHub repo path. If not, decide: rewrite the module path OR split the Go SDK to its own repo.
- Confirm `go get <module>@main` works from a clean machine (modulo the absence of a tag)
- Write the tag command + verify script into `docs/runbooks/publish-go-sdk.md` so T+14 is literally one command

### Shared
- Add a line to the V1.0 HN post and blog: **"Python and Go SDKs land next week and the week after."** This converts deferral from weakness to roadmap. HN readers reward specificity.

---

## 7 — The one scenario that flips this

Ship PyPI in V1.0 if — and only if — **a Python-first launch partner commits to amplifying the release on the same day**. Examples:
- ElizaOS maintainer tweets "we now support paying Solvela via `pip install solvela-sdk`"
- LangChain adds Solvela as a provider in their docs
- A well-known Python agent-dev influencer plans a video

Without that, Python in V1.0 adds surface area for no audience uplift. With it, Python is the narrative and npm is the support act.

---

## 8 — Decision checklist

Circle one for each before Phase 4 launch:

- [ ] **PyPI in V1.0** — only if a Python amplification partner is locked in
- [x] **PyPI deferred to T+7** — recommended default
- [ ] **Go in V1.0** — skip; Go does not move V1.0 numbers
- [x] **Go deferred to T+14** — recommended default
- [ ] **crates.io `solvela-cli`** — skip for V1.0; revisit T+30 based on adoption

If you pick the recommended defaults, your Phase 4 runbook is unchanged. You just pre-stage PyPI and Go during V1.0 prep so T+7 and T+14 are one-command shipments.
