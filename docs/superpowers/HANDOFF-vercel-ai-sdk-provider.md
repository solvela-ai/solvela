# Session handoff — 2026-04-18 (supersedes 2026-04-16)

## Where we are

Phases 1–9 of `docs/superpowers/plans/2026-04-16-vercel-ai-sdk-provider-plan.md` (rev 4.1) are **shipped and committed on branch `feat/landing-one-pager`**. All five user-side Phase 10 gates are satisfied. Remaining work: Phase 11 (live devnet smoke) then Phase 10 (publish).

## What landed this session

Three commits on `feat/landing-one-pager`:

| SHA | Phase | Summary |
|---|---|---|
| `84aeea6` | 1–7 | scaffold + provider factory + fetch wrapper + reference adapter + codegen + typed errors + 257 unit tests |
| `712bf5a` | 8 | 13 mocked-gateway integration scenarios + shared `mock-gateway.ts` helper |
| `d083c84` | 9 | README + examples + Fumadocs MDX + key-leak guard; plus 6 docs fixes from the review pass |

**Test health (latest):** 23 files / 349 tests passing in ~4.2 s. `npm run check:docs` clean. `npm audit --production` reports 0 vulnerabilities. `npm pack --dry-run` produces a 47.9 kB tarball with only `dist/`, `examples/`, `README.md`, `LICENSE`, `package.json` — no secrets, no tests, no scripts.

**Bundle hygiene verified:** `grep -E "(Keypair|spl-token|bs58|createPaymentHeader|secretKey)" dist/index.js` returns zero matches. Main entry tree-shakes clean; key-material lives only in `dist/adapters/local.js`.

## Phase 10 gates — all done

1. **`@solvela` npm org** — converted from the `solvela` user account; personal account is now org owner.
2. **Automation token** — 90-day granular token, scope `@solvela`, skip-2FA, stored in the GitHub repo as `NPM_TOKEN`. Rotation reminder due **~2026-07-17**.
3. **GitHub OIDC repo setting** — verified; `id-token: write` will be declared per-workflow when the Phase 10 publish flow gets written.
4. **GPG signing key** — RSA 4096, key ID `702398150F21DFFE`, fingerprint `3F119E0683F30EE4F1154FAD702398150F21DFFE`, email `kd@sky64.io`, no passphrase, expires **2028-04-17**. Uploaded to `github.com/settings/keys`. Git config set: `user.signingkey = 702398150F21DFFE`, `tag.gpgsign = true`. `commit.gpgsign` intentionally NOT enabled (only tags need signing per plan).
5. **GitHub PAT** — `public_repo` scope, 30-day expiry, saved at `/tmp/solvela-gh-pat.txt` (40 bytes, mode 600, no trailing newline). **WARNING: `/tmp/` is tmpfs on most configs — a reboot will wipe it.** If rebooting before Phase 10 executes, re-mint and re-save.

## Known environment artifacts

- `/tmp/solvela-gh-pat.txt` — ephemeral PAT; survives only until reboot.
- `/tmp/solvela-gpg-public-key.asc` — copy of the GPG public key block (also in the key's `--armor --export`).
- `~/.gnupg/openpgp-revocs.d/3F119E0683F30EE4F1154FAD702398150F21DFFE.rev` — **needs off-machine backup**. This file is what revokes the key if compromised; if this machine is wiped without it, the key can't be cleanly revoked on keyservers.
- Git global config mutations: `user.email` corrected from `kdsky64.io` → `kd@sky64.io` (missing `@` was a typo); `user.signingkey`, `tag.gpgsign` set.

## Immediate next step on resume

**Phase 11 — live devnet smoke test.** User explicitly paused here to let adjacent work (docs-site migration?) complete before running the live test. On resume:

1. Confirm adjacent work is done and safe to continue.
2. Provision a funded Solana **devnet** keypair (not mainnet):
   - ≥ 0.10 devnet USDC-SPL via https://faucet.circle.com
   - ≥ 0.05 devnet SOL via https://faucet.solana.com or `solana airdrop 1`
   - Save somewhere readable (same `/tmp/` pattern as the PAT works).
3. Per plan §6 Phase 11:
   - `docker compose up -d` (Postgres + Redis + gateway).
   - Configure `SOLVELA_SOLANA_RPC_URL` + devnet keypair env.
   - `generateText({ model: solvela('anthropic-claude-sonnet-4-5'), prompt: 'Echo: hello' })` → expects 402 → signed tx → 200.
   - Same for `streamText`.
   - Abort-mid-retry scenario: `AbortController.abort()` right after signer returns; verify no double-spend on-chain via `getSignatureStatuses` / block explorer; verify warn-once fires with no signature bytes.
   - Gateway logs show the Solana signature, not a stub.
4. Attach tx signatures + block-explorer screenshots to completion evidence.

## Phase 10 — after 11 passes

Not yet written: the CI publish workflow. Plan §6 Phase 10 WI-5 specifies it: tag-push-triggered GitHub Action running `npm ci` → `npm run build` → `npm test` → `npm run size` → `npm publish --access public --provenance` → tarball-SHA verification. Will be authored by an executor subagent when we start Phase 10.

Phase 10 WI-6/7 uses the PAT from `/tmp/solvela-gh-pat.txt`: fork `vercel/ai`, branch `community-provider-solvela`, add `content/docs/providers/03-community-providers/solvela.md` from the README, open PR.

## Known plan gaps (non-blocking, flag for 0.2)

- **`SOLVELA_TIMEOUT_MS`** — listed in plan §5.2 but never implemented in `src/config.ts`. Either remove from plan or implement in a 0.2 bump.
- **CI Node 18 matrix gap** — `install` job is matrixed Node 18+20; `typecheck`, `lint`, `test-unit`, `test-integration`, `size`, `guard-install-scripts`, `audit` all hardcode Node 20. Means Node 18 runtime regressions could ship undetected. Fix before Phase 10 publish: extend the matrix to `test-unit` at minimum.
- **Deferred polish items from review rounds:** F3 (throwing-logger wrap), F4 (header-projection guard post-release), F7 (`url: ''` in parse-402 errors), F10 (`as` cast in provider.ts `fetch` plumbing), security M-2 (base64 not in redact regex). All documented, none safety-critical.
- **Phase 5 drift-guard** works but depends on the committed `src/generated/models.ts` being regenerated when `config/models.toml` changes. No auto-sync; manual `npm run generate-models` + commit is the process.

## Branch state

- Current branch: `feat/landing-one-pager` (NOT `main` — the three session commits sit here).
- Untracked items in repo root unrelated to this initiative: `docs/plans/claude-mem.md`, `docs/strategy/`, `pics/`, `sdks/typescript/solvela-sdk-0.1.0.tgz` (artifact left by Phase 1 spike — can delete).

## Plugin roadmap order (unchanged)

1. **Vercel AI SDK provider** — 🟢 Phases 1–9 shipped; 11 + 10 remaining.
2. **LangChain adapter** — next; apply the template extracted from §13 of the current plan.
3. **Drop-in OpenAI SDK shims** — one per language (Py/TS/Go).
4. **Go SDK signing** — plan exists at `docs/superpowers/plans/2026-04-10-go-sdk-signing-support.md`.

## Process rules in effect (unchanged from 2026-04-16 handoff)

1. Research first, via specialized skills/docs — no memory-based guessing.
2. Plan = 100 % coverage (tech, env vars, creds, skills/hooks/MCPs, manual actions, infra).
3. Specialist agents only — never generalists where a specialist exists.
4. All code delegated — main agent plans + reviews, never writes code. Exception on this pass: I hand-ran `git add` / `git commit` / small bash verifications; no TypeScript or test files were hand-written.
5. If a subagent is blocked, surface to user; do not pick up the pen.
6. Quality > speed, always.
7. Verification before completion; evidence > assertion.

## Caveats for the next session

- **Don't assume `/tmp/solvela-gh-pat.txt` still exists.** Check `ls /tmp/solvela-gh-pat.txt && wc -c /tmp/solvela-gh-pat.txt` first (expect 40 bytes). If gone, mint a new PAT.
- **Don't publish from a laptop.** Plan §2 + §8 Sec-20 mandates CI-only publish via the tag-push workflow.
- **The Phase 10 publish workflow isn't written yet.** It gets authored as part of executing Phase 10 WI-5, not before.
- **Branch is `feat/landing-one-pager`, not `main`.** Decide on rebase/merge strategy before Phase 10 tags anything — you probably want these commits landed on `main` first, then tag `sdks/ai-sdk-provider/v0.1.0` against `main`.
