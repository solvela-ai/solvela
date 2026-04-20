# Solvela Private Testing — Internal Docs

This directory contains the private testing documentation package for the Solvela MCP plugin launch (MCP server, CLI installer, OpenClaw provider).

## Files

- **`tester-guide.md`** — Tester-facing quickstart. Give this to external testers. Covers install, first test, what to try, and feedback.
- **`test-wallet-setup.md`** — How to create and fund a dedicated test wallet. Testers should follow this before starting.
- **`feedback-template.md`** — Structured feedback capture form. Testers fill this in for each issue or observation.
- **`internal-runbook.md`** — Internal-only. Who does what when testers report issues. Severity rubric, assignment, SLA, graduation criteria.
- **`invitation-email.md`** — Draft invitation email. Customize with tester names, duration, access method, and launch timeline.
- **`graduation-checklist.md`** — Checklist to verify before flipping to public launch. All must-haves + verification steps.

## Quick checklist before inviting testers

1. [ ] Recruit 3–5 testers (mix of Claude Code, Cursor, OpenClaw if possible)
2. [ ] Give each tester a copy of `test-wallet-setup.md` + `tester-guide.md`
3. [ ] Set up feedback intake channel (GitHub issues in private repo, email, Slack, etc.)
4. [ ] Assign an owner to triage feedback daily (see `internal-runbook.md`)
5. [ ] Schedule 2–3 checkpoints during testing window (e.g. week 1, mid-week, end of week)
6. [ ] Plan 1–2 hour turnaround for CRITICAL issues (payment failures, security issues)
7. [ ] Prepare a "what's next" communication for testers when testing closes

## Testing timeline (typical)

- **Days 1–3:** Initial installs, first few chats, basic tool exploration
- **Days 4–7:** Smart routing profiles, escrow mode (if enabled), edge cases
- **Days 8–10:** Cleanup, final feedback, graduation decision

Target 2–4 hours of tester time over 2 weeks.

## Success signal

Zero unresolved CRITICAL/HIGH issues, positive aggregate feedback sentiment, and ≥3 independent testers able to complete README cold without agent help.
