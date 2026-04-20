# Solvela MCP Plugin — Tester Invitation Email

**To:** [TESTER_NAME]  
**Subject:** Early access: Test Solvela's new MCP plugins (earn $X credits)

---

Hi [TESTER_NAME],

We're launching a major feature for Solvela and we'd like you to be an early tester.

## What is Solvela?

Solvela is a Solana-native payment gateway for AI agents. Instead of API keys, agents pay per-call with USDC. We're shipping MCP (Model Context Protocol) plugins so Claude Code, Cursor, and OpenClaw users can access Solvela transparently—no setup, just USDC in your wallet.

## Why we're inviting you

[Personalize: You understand Solana / you're active in AI tooling / you've given us great feedback before / you're testing [Host] which is a priority focus, etc.]

## What you'll test

Three packages shipping together:

1. **MCP Server** — Claude Code, Cursor, Claude Desktop, OpenClaw tools for chat, smart routing, wallet status, model listing, spending tracking
2. **CLI Installer** — One-command setup: `solvela mcp install --host=cursor`
3. **OpenClaw Provider Plugin** (opt-in) — Solvela models in OpenClaw's native picker

**Time commitment:** 2–4 hours over 2 weeks. Estimate: 30 min setup, then ~20 min every few days to explore and report back.

**Testing window:** [START_DATE] to [END_DATE]

## What you get

- **$[AMOUNT] in USDC credits** to cover all testing costs (you won't spend more than $0.10)
- **Namecheck in our launch blog post** (if you want)
- **Direct influence on the roadmap** — your feedback shapes what ships next
- **Early access** before public launch (timing TBD, likely 2–4 weeks after this test phase)
- **[Optional: Free 6 months of Solvela Premium credits]** (if that exists)

## How to participate

1. **Accept or decline:** Reply to this email or comment in [FEEDBACK_CHANNEL] by [DATE].
2. **Set up your test wallet:** Follow `docs/private-testing/test-wallet-setup.md` (15 min). Create a dedicated throwaway Solana wallet on Mainnet with ~$5 USDC. Never use your personal wallet.
3. **Install Solvela:** Use our CLI installer or manual config. Docs: `docs/private-testing/tester-guide.md`. (15 min)
4. **Test:** Make ~10–15 calls, try different profiles, report any issues.
5. **Feedback:** Fill out `docs/private-testing/feedback-template.md` for each issue or observation.

## Confidentiality

- **Repo access** is private. Do not share the GitHub repo URL, npm package names (they're not public yet), or tester docs publicly.
- **Your wallet activity** is public on Solana Explorer (that's blockchain). Keep it throwaway.
- **Feedback** stays in our private feedback channel—no screenshots, data, or details shared externally until we launch.

Breach of confidentiality may affect future early access programs. We trust you to help us ship thoughtfully.

## Support

- **Questions during setup?** Reply here or ping us in [SLACK_CHANNEL].
- **Found a bug?** Fill out the feedback form. We'll triage within 24 hours.
- **Security issue?** Email [SECURITY_EMAIL] immediately (not in feedback form).

## FAQ

**Q: Will I get charged?**  
A: No. We're providing $[AMOUNT] credits. If you somehow exceed it (unlikely—testing costs ~$0.01–$0.10), we'll cover it.

**Q: What if the gateway crashes during testing?**  
A: We're monitoring uptime closely. If there's extended downtime, we'll extend the testing window by the same duration.

**Q: Can I keep using Solvela after the test ends?**  
A: Yes, but prices will be different. We'll announce pricing before public launch. You get early-access pricing of [PRICING] (if applicable).

**Q: Is this production-ready?**  
A: It's v1.0-candidate. We've done internal QA, but private testing helps us catch real-world issues before public launch.

---

**Ready to help shape the future of Solana AI payments?**

[BUTTON: Accept] [BUTTON: Decline]

Or reply to this email.

---

**Important links for after you accept:**
- Test wallet setup: `docs/private-testing/test-wallet-setup.md`
- Tester quickstart: `docs/private-testing/tester-guide.md`
- Feedback form: `docs/private-testing/feedback-template.md`
- Feedback channel: [SLACK_CHANNEL] or GitHub issues
- Questions/support: [CONTACT_EMAIL] or reply here

---

Thank you for being part of our mission to make agent-to-LLM payment flows as simple as Solana wallets.

— [YOUR_NAME]  
Solvela Team

P.S. If you know someone else who'd be a great tester (especially on [Host], [Host], etc.), send them my way. We're inviting a small cohort this round.
