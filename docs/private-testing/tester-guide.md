# Solvela MCP Plugin — Tester's Quickstart

Thank you for testing Solvela's new MCP (Model Context Protocol) plugin ecosystem. This guide walks you through install, first test, and what to explore.

## What is Solvela?

Solvela is a Solana-native payment gateway for AI agents. Instead of API keys, agents pay per-call with USDC on Solana via the x402 protocol. The MCP plugin lets Claude Code, Cursor, and other AI hosts call Solvela tools transparently.

## What you're testing

We're shipping three things:

1. **MCP Server** (`@solvela/mcp-server`) — Exposes Solvela as tools in Claude Code, Cursor, Claude Desktop, OpenClaw
2. **CLI Installer** (`solvela mcp install`) — One-command setup for each host
3. **OpenClaw Provider Plugin** (`@solvela/openclaw-provider`) — First-class LLM provider in OpenClaw (not a tool)

You'll test at least two of these depending on your host.

## Prerequisites

- **Solana wallet:** You need a throwaway wallet with ~$5 USDC on Mainnet (never use a personal wallet). See `test-wallet-setup.md` for exact steps.
- **MCP-capable host:** Claude Code, Cursor, Claude Desktop, or OpenClaw.
- **Base58 keypair secret:** You'll need this in `SOLANA_WALLET_KEY` env var. See wallet setup guide.
- **2–4 hours over 2 weeks:** Initial setup, light exploration, feedback.

## Installation

### Option A: Via CLI installer (recommended)

```bash
# Install the Solvela CLI
npm install -g @solvela/cli

# Pick your host and run the installer
solvela mcp install --host=claude-code      # Claude Code
solvela mcp install --host=cursor           # Cursor
solvela mcp install --host=claude-desktop   # Claude Desktop
solvela mcp install --host=openclaw         # OpenClaw (MCP)
```

The installer writes the config and prints a reminder to set your wallet key:

```bash
export SOLANA_WALLET_KEY=<your-base58-keypair>
```

**CLI options:**

- `--budget=2.00` — Set session spending limit (default unlimited)
- `--signing-mode=auto` — `auto` (default), `escrow`, `direct`, or `off`
- `--dry-run` — Preview config without writing
- `--uninstall` — Remove Solvela from your host config

### Option B: Manual JSON copy-paste (fallback)

See `README.md` in `sdks/mcp/` for exact JSON snippets per host. Not recommended—use the installer.

### Option C: OpenClaw Provider Plugin

If you're testing OpenClaw:

```bash
npm install @solvela/openclaw-provider
```

Then set env vars and OpenClaw will show "Solvela" in the model picker (not as a tool).

## Your first test

Once installed, you have a **5-tool MCP server**:

### 1. `chat` — Talk to a specific model

Ask your host: "Use the `chat` tool. Model: `anthropic/claude-opus-4-1`. Prompt: What is x402?"

Expected: The model calls the tool, signs a transaction, sends to the gateway, and returns a response. Cost: ~$0.003 (cheap).

### 2. `smart_chat` — Auto-pick the cheapest capable model

Ask: "Use `smart_chat` with profile `eco`. Prompt: What is 2+2?"

Expected: The gateway analyzes your prompt, routes to the cheapest model (likely Claude Haiku), and returns the answer. Cost: ~$0.0005.

### 3. `wallet_status` — Check your wallet and gateway health

Ask: "Call `wallet_status`."

Expected: You see your wallet address, gateway health, Solana RPC status, and current session spending.

### 4. `list_models` — Browse available models

Ask: "Call `list_models` with filter: `claude`."

Expected: You see all Claude models with their USDC price per million tokens (input + output).

### 5. `spending` — Monitor your spend

Ask: "Call `spending`."

Expected: You see total requests, USDC spent, remaining budget (if you set one), and wallet address.

## What to try next

Once your first test works:

1. **Multiple routing profiles** — Try `smart_chat` with `eco`, `auto`, `premium`, `free` on the same prompt. Compare costs + quality.

2. **Budget enforcement** — Set `SOLVELA_SESSION_BUDGET=0.50` and keep chatting until the tool refuses with a budget-exceeded error.

3. **Escrow mode (if enabled)** — If you see `SOLVELA_ESCROW_MODE=enabled` in your config, try:
   - Call `deposit_escrow` with `amount_usdc: "2.00"`
   - Make several `chat` calls
   - Check `spending` to see the escrow balance deplete

4. **Edge cases:**
   - Very long prompts
   - Streaming vs. non-streaming responses
   - Fast back-to-back calls (concurrency)
   - Errors (set a bad wallet key, try invalid model, etc.)

5. **OpenClaw-specific:** If you're on OpenClaw Provider Plugin, pick "Solvela Auto" in the model picker and chat normally. The plugin signs transparently.

## Spending safety

All costs are denominated in USDC on Solana. Testing costs ~$0.01–$0.10 per tester over 2 weeks. We cover this.

**Budget enforcement is real:** If you set `SOLVELA_SESSION_BUDGET=1.00`, you cannot spend more than $1.00 in that session. The tool will refuse with a clear error.

## Security reminders

- **Use a hot wallet created for testing only.** Never use a personal wallet with real assets.
- **`SOLANA_WALLET_KEY` is a secret.** Never commit it to version control. Use `chmod 0600` on any env file containing it.
- **Escrow mode is optional.** It's only available if `SOLVELA_ESCROW_MODE=enabled` in your config. If enabled, the AI model can call `deposit_escrow`—capped at $5 per deposit and $20 per session.
- **Wallet address is public.** Publish your spend on Solana Explorer for transparency if you want.

## Troubleshooting

| Issue | Fix |
|-------|-----|
| Tool doesn't appear in host | Restart your host. Check config file was written (see `--dry-run` output). Set `SOLANA_WALLET_KEY` env var. |
| "Invalid wallet key" error | Ensure `SOLANA_WALLET_KEY` is base58-encoded, 88 characters. See `test-wallet-setup.md`. |
| "Budget exceeded" | You've hit your `SOLVELA_SESSION_BUDGET` limit. Call `spending` with `reset: true` to clear the session. |
| "Gateway offline" or 502 | The Solvela gateway may be down. Check `wallet_status` tool. Check status page. |
| Slow responses | Gateway may be under load. Check your RPC URL in env. Solana network may be congested. |

## Feedback

When you hit an issue, unclear UX, or positive observation, fill out `feedback-template.md` and send it to [feedback contact — see invitation email].

Include:

- What you were doing
- What you expected
- What actually happened
- Wallet address (public key only)
- Any error messages or logs

## Cleanup

When testing is done:

1. **Delete your test wallet** (or consider it burned—never reuse it):
   ```bash
   rm ~/.solvela/test-keypair.json
   ```

2. **Remove Solvela from your host**:
   ```bash
   solvela mcp uninstall --host=claude-code
   ```

3. **Clear session file**:
   ```bash
   rm ~/.solvela/mcp-session.json
   ```

## Questions?

Reach out via [feedback contact]. We're here to help.

Thank you for testing!
