# Solvela Private Testing — Internal Runbook

This runbook defines how the team processes tester feedback, prioritizes fixes, and determines when testing is "done."

## Feedback intake

**Where feedback goes:**
- [Define: GitHub private repo issue, Slack channel, email inbox, etc.]
- Example setup: `solvela/private-testing` private repo with an issue template

**Daily triage:** Assign one team member as the **triage owner** to review new feedback within 24 hours.

---

## Severity rubric

| Severity | Criteria | Response SLA | Example |
|----------|----------|--------------|---------|
| **CRITICAL** | Payment verification fails, private key leaks, wallet drained, gateway unreachable | 2 hours | "Signing always fails with 'Invalid transaction'" |
| **HIGH** | Tool doesn't work, UX is broken, data loss risk, security weakness | 4 hours | "Wallet status returns 500", "Escrow deposit doesn't cap correctly" |
| **MEDIUM** | Feature works but UX is bad, edge case broken, documentation unclear | 24 hours | "Error message is cryptic", "OpenClaw plugin setup took 45 min" |
| **LOW** | Typo, nit, cosmetic issue, nice-to-have | No SLA | "Button text should be 'Copy Address' not 'Copy Wallet'" |

---

## Triage workflow

1. **Severity assignment:** Triage owner tags each issue with a severity label.
2. **Assignment:** Route to the responsible subsystem owner (see below).
3. **Initial response:** Comment with a reproducibility check or quick fix within SLA.
4. **Re-triage if needed:** If the issue is more/less severe than initially tagged, move it.

**Escalation:** Any CRITICAL issue goes to a Slack #critical-incidents channel immediately. Ping the on-call engineer.

---

## Subsystem ownership

| Subsystem | Owner | Issues |
|-----------|-------|--------|
| MCP server (`@solvela/mcp-server`) | [name] | Tool failures, budget enforcement, session persistence |
| CLI installer (`solvela mcp install`) | [name] | Config generation, host-specific bugs, uninstall issues |
| OpenClaw Provider Plugin (`@solvela/openclaw-provider`) | [name] | wrapStreamFn hook failures, model picker issues |
| Gateway (`crates/gateway`) | [name] | 402 responses, payment verification, health checks |
| Test wallet setup / docs | [name] | Documentation clarity, missing steps |

---

## Common issues & quick fixes

| Issue | Fix | Effort |
|-------|-----|--------|
| "Tool not in host UI" | Restart host. Check env vars are set. Check config file syntax. | ~5 min |
| "Invalid base58 key" error | Verify key is 88 chars, no spaces, all base58 characters. Use `solana-keygen show --format base58-secret`. | ~10 min |
| "Budget exceeded immediately" | Check if session file exists and is stale. Delete `~/.solvela/mcp-session.json` and retry. | ~5 min |
| Slow chat responses | Check Solana RPC URL is correct. Check gateway `/health`. Network may be congested. | ~10 min |
| OpenClaw plugin fails to load | Ensure `@solana/web3.js` is installed. Check Node version ≥18. | ~15 min |

---

## Response template

Use this when first responding to a tester:

```
Hi [tester],

Thanks for reporting. I'm looking into [brief summary of issue].

Quick questions:
- [Clarify any ambiguity from the report]

If you could run this for me:
[Command to gather more data, e.g., check env vars, paste error message]

I'll follow up in [SLA] with either a fix or a workaround.
```

---

## Fix & release

### For CRITICAL / HIGH issues:

1. **Root cause analysis:** Subsystem owner investigates.
2. **Fix development:** Small targeted fix, tested locally.
3. **Tester validation:** Cherry-pick fix to a test branch. Ask the original reporter to verify before merging to main.
4. **Release:** Publish a patch version ASAP.
   - `@solvela/mcp-server`: `npm publish` with patch bump
   - CLI: `solvela-cli` v1.0.1 via `cargo dist`
   - Provider Plugin: `npm publish` patch

### For MEDIUM / LOW issues:

1. Queue for the next scheduled release.
2. Batch multiple issues if possible.
3. Release weekly or bi-weekly (user decision).

---

## Testing metrics (track weekly)

| Metric | Target | How to measure |
|--------|--------|-----------------|
| **Active testers** | ≥3 (1 per host) | Count unique tester feedback entries |
| **Payment success rate** | >99% | Count successful chat calls / total attempts |
| **Average response time to CRITICAL** | <2 hrs | Check creation → first response timestamps |
| **Gateway uptime** | >99% | Check `GET /health` logs |
| **Tool-call latency** | <5 sec p95 | Sample response times from tool logs |
| **Issues resolved** | — | Count CRITICAL/HIGH moved to "done" |

**Weekly sync:** Every Tuesday (or pick a day), review metrics with the team. If uptime <99%, pause new tester signups; focus on stability.

---

## Go/no-go decision gates (graduation)

Testing is "done" when ALL of the following are true:

| # | Criterion | How verified |
|---|-----------|--------------|
| 1 | ≥3 independent testers complete the full flow (chat + smart_chat + wallet_status + spending) | Feedback forms submitted showing all 5 tools tested |
| 2 | Zero unresolved CRITICAL issues | GitHub issues labeled CRITICAL all closed |
| 3 | Zero unresolved HIGH issues OR documented workarounds with tester sign-off | HIGH issues closed or marked "accepted limitation" with tester agreement |
| 4 | Payment verification error rate <1% | Logs show successful 402 → sign → retry flow >99% of calls |
| 5 | Gateway uptime >99% during test window | Heartbeat logs or status page shows <1 hour downtime |
| 6 | Aggregate feedback sentiment positive | Tester summaries average ≥4/5 stars and say "would use in production" |
| 7 | ≥3 testers complete README cold with no agent assistance | Feedback forms show at least 3 testers used only the written guide |
| 8 | Escrow mode validated (if enabled) | At least 1 tester successfully deposits and claims escrow, or escrow is disabled for this test cycle |
| 9 | Security review passed | `pr-review-toolkit:silent-failure-hunter` + manual key-redaction audit complete |
| 10 | User has 24–48 hour post-launch response window blocked | Calendar shows engineering capacity for rapid fixes immediately after public release |

**Escalation:** If any criterion is not met by the scheduled end date, decide: (a) extend testing 1 week, (b) pause that feature (e.g., escrow mode), or (c) document the issue as "known limitation — will fix in v1.0.1" with a specific owner and timeline.

---

## Post-launch support (week 1)

Public launch goes live. The team stays on alert for 5–7 days:

- **On-call rotation:** Rotate 24 hr shifts (or pick business hours if launch is US/EU only).
- **Response SLA:** CRITICAL issues get initial response within 1 hour.
- **Patch releases:** Prepare hotfixes for the first 3 days. Then switch to normal weekly cadence.
- **Comms:** Post a launch blog, tweet, and HN thread. Monitor replies for early issues.

---

## Documentation ownership

Before public launch, verify:

- [ ] README is accurate and tested by ≥1 human (not AI)
- [ ] Quickstart examples work end-to-end
- [ ] Troubleshooting section covers the top 5 issues from private testing
- [ ] Security section is prominent and clear
- [ ] Links to external docs (Solana, MCP spec, etc.) are current
- [ ] Code examples are syntax-correct (no typos, real CLI commands)

---

## Communication template (for testers when testing closes)

```
Hi [Tester Group],

Thank you for testing Solvela's MCP plugin over the past [2 weeks]. Your feedback was invaluable.

Here's what we're shipping based on your reports:
- [Fix A]: payment verification robustness
- [Fix B]: OpenClaw plugin startup error handling
- [Feature C]: budget reset command

We've scheduled the public launch for [DATE]. You'll get early access to [benefit: free credits, namecheck in launch post, etc.].

If you want to continue testing the latest builds or shape the roadmap, let us know. We're building this in public.

Thank you again for believing in Solvela.
— The Solvela team
```

---

## Checklist for "ready to flip public"

- [ ] All go/no-go gates (above) are satisfied
- [ ] Team has confirmed no CRITICAL / HIGH issues remain
- [ ] npm packages are staged (not published)
- [ ] Blog post drafted and scheduled
- [ ] HN / Twitter posts drafted
- [ ] Anthropic MCP Registry submission is queued
- [ ] cursor.directory PR is ready
- [ ] OpenClaw docs PR is ready
- [ ] On-call rotation is assigned
- [ ] Triage owner handoff document is ready
- [ ] Tester thank-you email is sent (see template above)

Once checked, the user gives final approval. Then: `npm publish`, tag release, post the launch thread.
