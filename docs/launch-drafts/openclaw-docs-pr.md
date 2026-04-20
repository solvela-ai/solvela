# OpenClaw Docs PR Draft — Solvela Integration

**Status:** DRAFT. Do NOT submit. User triggers after private testing.

---

## PR Target

Upstream repo: Likely `https://github.com/openclaw-ai/docs` (or similar)  
Target file: `docs/integrations/providers/solvela.md` (or `docs/plugins/solvela.md`)

---

## PR Body

```markdown
## Add Solvela to OpenClaw Integrations

Adds a new integration guide for Solvela, a pay-per-call LLM gateway on Solana with x402 protocol support.

This allows OpenClaw users to:
- Route LLM calls through Solvela and pay transparently in USDC-SPL
- Use Solvela as a first-class model provider (via `@solvela/openclaw-provider`) or as an MCP tool
- Enable trustless escrow deposits for pay-only-for-what-you-receive guarantees
- Avoid API keys, accounts, and per-user subscriptions

Includes setup instructions, config examples, first-call walkthrough, and links to full documentation.
```

---

## Proposed Docs Section

This content should be added to OpenClaw's integrations documentation in the style/tone of existing provider docs.

### File: `docs/integrations/solvela.md`

```markdown
# Solvela — x402 USDC LLM Gateway

Solvela is a Solana-native LLM payment gateway that lets agents pay for LLM calls with USDC-SPL via the x402 protocol. No API keys, no accounts, no per-user subscriptions — just direct on-chain settlement.

## What is Solvela?

Solvela bridges your agents to 26+ LLM models from 5 providers (OpenAI, Anthropic, Google, xAI, DeepSeek) with transparent per-call USDC-SPL settlement. Two integration options:

1. **Provider Plugin** (`@solvela/openclaw-provider`) — Solvela models appear in your model picker. Best for agents that should route to Solvela transparently.
2. **MCP Server** (`@solvela/mcp-server`) — Solvela appears as an MCP tool. Best for agents that decide when to use Solvela vs other providers.

## Installation

### Option 1: Provider Plugin (Recommended)

Install the provider plugin to register Solvela as a first-class LLM provider:

```bash
npm install @solvela/openclaw-provider
```

Add to your `openclaw.plugin.json`:

```json
{
  "plugins": {
    "solvela": {
      "package": "@solvela/openclaw-provider",
      "config": {
        "signingMode": "auto",
        "gateway": "https://api.solvela.ai"
      }
    }
  }
}
```

Set your wallet credentials:

```bash
export SOLANA_WALLET_KEY="your-base58-private-key"
export SOLANA_RPC_URL="https://api.mainnet-beta.solana.com"
```

### Option 2: MCP Server

Install as an MCP server:

```bash
npm install -g @solvela/mcp-server
openclaw mcp set solvela '{
  "command": "npx",
  "args": ["@solvela/mcp-server"],
  "env": {
    "SOLANA_WALLET_KEY": "your-base58-private-key",
    "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com",
    "SOLVELA_API_URL": "https://api.solvela.ai"
  }
}'
```

## Configuration

### Required Environment Variables

- **SOLANA_WALLET_KEY** (string, base58) — Your Solana wallet's private key. Used to sign transactions on-chain. Never commit to repos.
- **SOLANA_RPC_URL** (string, URL) — Solana RPC endpoint. Example: `https://api.mainnet-beta.solana.com`

### Optional Environment Variables

- **SOLVELA_API_URL** (string, URL) — Solvela gateway URL. Default: `https://api.solvela.ai`
- **SOLVELA_SESSION_BUDGET** (string, USDC amount) — Max spending for this session. Example: `10.00` (no limit if unset)
- **SOLVELA_SIGNING_MODE** (enum: `auto|escrow|direct|off`) — Payment signing mode:
  - `auto` (default) — Use escrow when available, fall back to direct transfer
  - `escrow` — Require escrow (pay-only-for-what-you-receive); error if gateway doesn't support
  - `direct` — Direct USDC transfer (no escrow)
  - `off` — Demo mode (no real payments)
- **SOLVELA_ESCROW_MODE** (enum: `enabled|disabled`) — Enable the `deposit_escrow` tool (provider plugin only). Default: disabled.

## First Call Example

### Using the Provider Plugin

```bash
openclaw chat --model solvela/claude-sonnet-4 "Write hello world in Rust"
```

OpenClaw will:
1. Send your prompt to Solvela's gateway
2. Solvela computes the cost (~$0.002 for this prompt)
3. Your wallet signs a USDC-SPL transaction
4. Payment is verified on-chain
5. The LLM response streams back to your agent
6. Cost breakdown is logged

**Response:**

```
Rust hello world program:

fn main() {
    println!("Hello, world!");
}

Cost breakdown:
- Input tokens: 5 @ $0.00001/token = $0.00005
- Output tokens: 15 @ $0.00002/token = $0.0003
- Subtotal: $0.00035
- Platform fee (5%): $0.0000175
- Total: $0.0003675
```

### Using the MCP Server

```bash
openclaw chat --tool solvela/chat "Write hello world in Rust" --option model=claude-sonnet-4
```

## Tools Available (MCP Server)

- **chat** — Send a prompt to a specific LLM model
- **smart_chat** — Route through Solvela's smart router (picks best model for the task)
- **list_models** — Show available models and per-token pricing
- **wallet_status** — Check your wallet balance, session spending, and escrow status
- **spending** — View cumulative spending with budget enforcement
- **deposit_escrow** — Top up your escrow deposit (when `SOLVELA_ESCROW_MODE=enabled`)

## Trustless Escrow

Solvela's killer feature: **pay only for what you receive**.

With escrow enabled, funds are locked in an Anchor program on Solana mainnet. The gateway claims USDC only when your LLM response completes. If the stream fails mid-response (network issue, provider timeout), your funds are refundable.

To enable escrow:

```bash
export SOLVELA_ESCROW_MODE=enabled
export SOLVELA_MAX_ESCROW_DEPOSIT=5.00    # Max per deposit (default)
export SOLVELA_MAX_ESCROW_SESSION=20.00   # Max cumulative per session (default)

openclaw chat --tool solvela/deposit_escrow '{"amount_usdc": "5.00"}'
```

This creates a session deposit. Your first N calls draw from this until it's exhausted or expires.

## Pricing

- **Platform fee:** 5% per call
- **No account requirement** — just sign transactions
- **Per-call settlement** — immediate on-chain
- **No minimum balance** — fund as you go

Example: A $1.00 LLM API call costs $1.05 total (API + 5% fee).

## Troubleshooting

### "Missing SOLANA_WALLET_KEY"

Wallet key not set. Export it before running OpenClaw:

```bash
export SOLANA_WALLET_KEY="your-base58-private-key"
```

### "Payment signature verification failed"

Your wallet may have signed a stale transaction. Retry — nonce pools recycle automatically.

### "Escrow expired"

Your escrow deposit expired (typically 1 hour). Call `deposit_escrow` again to top up.

### "Budget exceeded"

You've hit your `SOLVELA_SESSION_BUDGET` limit. Either raise the limit or start a fresh session.

## Full Documentation

- **Setup & API:** https://docs.solvela.ai/en/mcp
- **Architecture:** https://docs.solvela.ai/en/architecture
- **Blog & announcements:** https://solvela.ai/blog
- **GitHub:** https://github.com/solveladev/solvela

## Support

- **Issues:** https://github.com/solveladev/solvela/issues
- **Email:** support@solvela.ai
- **Discord:** [link to Solvela Discord, if exists]

---

## Next Steps

1. Install and set your wallet credentials (see Installation above)
2. Run `openclaw models list` to see Solvela models (provider plugin) or `openclaw mcp list` to see MCP servers
3. Send your first chat via `openclaw chat --model solvela/claude-sonnet-4 "your prompt"`
4. Check spending with `openclaw tool solvela/spending`

Happy agentic payments!
```

---

## Submission Checklist

- [ ] Confirm OpenClaw docs repo location and structure
- [ ] Research existing provider docs format (copy tone/style)
- [ ] Verify plugin install paths match `@solvela/openclaw-provider` readme
- [ ] Test all code examples locally before PR
- [ ] Create PR to upstream OpenClaw docs repo
- [ ] Request review from OpenClaw maintainers
- [ ] Incorporate feedback
- [ ] Merge and verify docs appear on docs.openclaw.ai
- [ ] Update solvela.ai/docs with link to OpenClaw integration guide
