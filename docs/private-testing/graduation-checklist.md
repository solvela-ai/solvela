# Solvela MCP Plugin — Graduation Checklist

Use this checklist to verify that testing is complete and the product is ready for public launch.

Print, tick off items as you verify them, and have the team lead sign off before flipping the switch.

---

## Pre-launch gates

### Tester participation

- [ ] **At least 3 independent testers** have completed the full flow (all 5 tools: chat, smart_chat, wallet_status, list_models, spending)  
  _How to verify:_ Check feedback forms; each should mention testing multiple tools
  
- [ ] **At least 1 tester per primary host** (Claude Code, Cursor, OpenClaw)  
  _How to verify:_ Feedback forms list the host; tester count by host ≥1 each

- [ ] **Testing window is ≥7 calendar days** (no shorter)  
  _How to verify:_ Check dates on first & last feedback entries

### Issue resolution

- [ ] **Zero unresolved CRITICAL issues**  
  _How to verify:_ GitHub issue backlog shows no open CRITICAL labels in `solvela/private-testing`

- [ ] **Zero unresolved HIGH issues** (or documented workarounds with tester agreement)  
  _How to verify:_ All HIGH issues are either closed or marked "acceptable limitation" with a comment from the original reporter saying "understood, acceptable"

- [ ] **All MEDIUM issues have a fix or a roadmap owner assigned for v1.0.x**  
  _How to verify:_ Each MEDIUM issue has a comment "fix in v1.0.1 (owner: @name)" or "deferred to v1.1: @name to spec"

### Payment reliability

- [ ] **Payment verification success rate >99%** across all testers  
  _How to verify:_ `gateway` logs show successful 402 → sign → retry flow on >99% of chat calls
  
  ```bash
  # Count successful 402 retries
  grep -c "payment verified" /var/log/solvela-gateway.log
  # Compare to total requests
  grep -c "POST /v1/chat" /var/log/solvela-gateway.log
  # Should be close to 100%
  ```

- [ ] **No reports of funds being lost or stranded**  
  _How to verify:_ Feedback forms show no entries saying "USDC disappeared" or "payment failed and money didn't return"; escrow refunds work if tested

### Stability

- [ ] **Gateway uptime >99% during testing window**  
  _How to verify:_ Status page or heartbeat logs show ≤14 minutes total downtime in the test period

- [ ] **No unplanned restarts or data corruption**  
  _How to verify:_ `crates/gateway` logs show clean startup; no panic/error logs relating to database or session state

- [ ] **MCP server and plugins handle restarts gracefully**  
  _How to verify:_ Feedback: "Restarted [host] mid-session and tools reappeared" (no manual reinstall needed)

### Feedback sentiment

- [ ] **Average tester rating ≥4/5 stars**  
  _How to verify:_ Feedback forms' "Overall impression" section; sum and divide by number of testers
  
  Example: 3 testers give 5, 4, 4 → average 4.33 ✓

- [ ] **≥50% of testers say "Yes, definitely" to production use**  
  _How to verify:_ Feedback forms' "Would you use in production?" section

- [ ] **Aggregate tone is positive or constructive** (no "this is broken and unsafe" themes)  
  _How to verify:_ Read the "biggest pain point" sections; issues should be specific UX complaints, not fundamental breakage

### Documentation quality

- [ ] **≥3 testers successfully followed the README cold** (no agent/claude-code help during install)  
  _How to verify:_ Feedback forms mention "followed README only" or no mention of using Claude Code for install help

- [ ] **Zero testers reported "missing step" or "docs are wrong"** (or issues were fixed)  
  _How to verify:_ No MEDIUM feedback entries of type "documentation"

- [ ] **Security section is prominent and clear**  
  _How to verify:_ At least 1 tester mentions seeing and understanding the key storage recommendations

### Escrow mode (if enabled)

- [ ] **At least 1 tester successfully deposits and claims escrow** (or escrow is explicitly disabled for this test cycle)  
  _How to verify:_ Feedback form or logs show successful `deposit_escrow` + `chat` call sequence with escrow claimed

- [ ] **Session cap enforcement is tested** (cumulative $20 limit per session survives restart)  
  _How to verify:_ Test: call `deposit_escrow` 4× $5, kill server, restart, attempt 5th $5 → must be rejected

- [ ] **No escrow funds are stranded** (no PDA lockups, refunds work on timeout)  
  _How to verify:_ `deposit_escrow` feedback shows no "USDC stuck in escrow" reports

---

## Infrastructure & security

### Code quality

- [ ] **Security review passed** (`pr-review-toolkit:silent-failure-hunter` + manual audit)  
  _How to verify:_ Signed-off security review doc exists; no comments like "redaction incomplete" or "key leak risk"

- [ ] **No hardcoded secrets or test credentials left in code**  
  _How to verify:_ `grep -r "STUB_BASE64_TX" sdks/mcp src/` returns zero results

- [ ] **Key redaction is verified** (private key bytes never appear in logs, errors, or responses)  
  _How to verify:_ Inject a known test key, run a chat call, grep stderr/logs for first 8 chars of the key → should return nothing

### Build & release

- [ ] **`npm publish @solvela/mcp-server@1.0.0` is staged and ready** (not yet published; version bumped to 1.0.0)  
  _How to verify:_ `npm view @solvela/mcp-server@1.0.0` throws 404 (not published yet)

- [ ] **`@solvela/openclaw-provider@1.0.0` is staged** (if shipping in same launch)  
  _How to verify:_ Version string in `sdks/openclaw-provider/package.json` is 1.0.0

- [ ] **`solvela-cli` v1.0.0 is staged** (cargo binary ready, not yet released)  
  _How to verify:_ `crates/cli/Cargo.toml` shows version = "1.0.0"

- [ ] **GitHub release notes are drafted**  
  _How to verify:_ File `RELEASE_NOTES.md` or draft in repo wiki with v1.0.0 highlights

### Distribution checklist (Phase 4 gates)

- [ ] **npm `@solvela/mcp-server@1.0.0` published with OIDC provenance**  
  _How to verify:_ `npm view @solvela/mcp-server@1.0.0` shows v1.0.0 on npmjs.com

- [ ] **Anthropic MCP Registry submission queued or submitted**  
  _How to verify:_ GitHub PR or ticket showing MCP Registry submission

- [ ] **cursor.directory PR submitted or merged**  
  _How to verify:_ GitHub PR link to upstream `directories` repo

- [ ] **OpenClaw docs PR submitted or merged** (if applicable)  
  _How to verify:_ GitHub PR link to upstream `openclaw` docs repo

- [ ] **Blog post scheduled** (`solvela.ai/blog` or mirror)  
  _How to verify:_ Scheduled post date matches or is before release date

- [ ] **HN / Twitter / X thread drafted**  
  _How to verify:_ Drafts exist in shared doc or Slack thread

---

## Team readiness

### Support capacity

- [ ] **On-call rotation is assigned** for 7 days post-launch (or business hours if not 24/7)  
  _How to verify:_ Calendar shows owner + backup for each day

- [ ] **Triage owner is identified** and committed to <1 hour response on CRITICAL issues  
  _How to verify:_ Slack message or calendar block confirms coverage

- [ ] **Subsystem owners are assigned** (MCP server, CLI, OpenClaw plugin, gateway)  
  _How to verify:_ `internal-runbook.md` ownership table is filled in with real names

### Post-launch comms

- [ ] **Tester thank-you email is drafted** (see `invitation-email.md` template)  
  _How to verify:_ Draft exists and lists specific thank-yous to each tester

- [ ] **Post-launch response plan is documented**  
  _How to verify:_ `internal-runbook.md` "Post-launch support" section is reviewed and confirmed

- [ ] **User has 24–48 hour response window unblocked** after launch  
  _How to verify:_ Calendar shows no meetings/commitments for [launch-date] + 2 days

---

## Sign-off

Once all items above are checked:

**Testing lead:** _________________ (name) **Date:** _________

**Product / user approval:** _________________ (name) **Date:** _________

**Engineering lead:** _________________ (name) **Date:** _________

---

## If any item is not checkable:

1. **Extend testing** by 1 week (for tester participation or stability issues)
2. **Document as known limitation** (for MEDIUM issues; owner must commit to fix in v1.0.1)
3. **Disable the feature** (e.g., set `SOLVELA_ESCROW_MODE=disabled` if escrow isn't ready)
4. **Pause launch** (for CRITICAL issues; document root cause + fix timeline)

Once resolved, re-check the item and re-sign.

---

**Final decision:** Approved for public launch ✓ or Pause / remediate (document why).
