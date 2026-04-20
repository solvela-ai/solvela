# Cursor Directory Submission Draft — Solvela MCP Server

**Status:** DRAFT. Do NOT submit. User triggers submission after private testing.

---

## Submission Process

Cursor Directory is maintained at [cursor.directory](https://cursor.directory) (GitHub repo: [pontusab/cursor-directory](https://github.com/pontusab/cursor-directory)). 

Submit via: Pull Request to the upstream repo with:
1. New entry in the server registry (likely `servers.json` or similar YAML)
2. Required metadata and description
3. Optional: screenshot (1200x630 recommended for preview cards)

---

## Metadata

```yaml
# For cursor.directory registry submission
name: solvela
title: Solvela — x402 USDC LLM Gateway
description: Pay-per-call LLM access on Solana. Real USDC-SPL settlement with trustless escrow. One-line install into Cursor.
homepage: https://solvela.ai
documentation: https://docs.solvela.ai/en/mcp
repository: https://github.com/solveladev/solvela
author: Solvela
license: Apache-2.0
tags:
  - ai
  - payments
  - blockchain
  - solana
  - llm-gateway
  - x402
category: payments
installation:
  type: npm
  command: npm install -g @solvela/mcp-server@latest
  alternativeCommand: solvela mcp install --host=cursor
requiredEnvironmentVariables:
  - SOLANA_WALLET_KEY
  - SOLANA_RPC_URL
optionalEnvironmentVariables:
  - SOLVELA_API_URL
  - SOLVELA_SESSION_BUDGET
  - SOLVELA_SIGNING_MODE
  - SOLVELA_ESCROW_MODE
  - SOLVELA_MAX_ESCROW_DEPOSIT
  - SOLVELA_MAX_ESCROW_SESSION
deeplink:
  scheme: cursor
  protocol: mcp+npm
  format: cursor://install?package=@solvela/mcp-server&env=SOLANA_WALLET_KEY,SOLANA_RPC_URL,SOLVELA_API_URL
```

---

## Cursor Deeplink

For the "Add to Cursor" button, the deeplink should look like:

```
cursor://install?package=@solvela/mcp-server&name=solvela&displayName=Solvela&env=SOLANA_WALLET_KEY,SOLANA_RPC_URL,SOLVELA_API_URL
```

When clicked, Cursor will:
1. Install `@solvela/mcp-server` from npm
2. Prompt user for required env vars
3. Write to `.cursor/mcp.json`
4. Reload the MCP server

---

## PR Body Template

```markdown
## Add Solvela MCP Server to Cursor Directory

Solvela is a production-grade MCP server for pay-per-call LLM access via the x402 protocol on Solana.

### What is Solvela?

- **One-line install** into Cursor, Claude Code, and Claude Desktop
- **Real USDC-SPL settlement** — no accounts, no API keys, no per-user subscriptions
- **Trustless escrow** — Anchor program deployed to Solana mainnet ensures pay-only-for-what-you-receive
- **Smart routing** — 15-dimension classifier picks the best model (OpenAI, Anthropic, Google, xAI, DeepSeek)

### Installation

```bash
solvela mcp install --host=cursor
```

Or manually:

```bash
npm install -g @solvela/mcp-server@latest
```

Then configure in `.cursor/mcp.json`:
- `SOLANA_WALLET_KEY` (your wallet's base58 private key)
- `SOLANA_RPC_URL` (e.g., https://api.mainnet-beta.solana.com)
- `SOLVELA_API_URL` (default: https://api.solvela.ai)

### Key Features

- **6 MCP tools:** chat, smart_chat, list_models, wallet_status, spending, deposit_escrow
- **Transparent pricing:** 5% platform fee, settled per-call
- **Escrow-first:** Default signing mode prefers escrow (pay-only-for-what-you-receive) when the gateway advertises it
- **Budget caps:** Session spending limits + per-deposit caps on escrow

### First Call Example

```
In Cursor, use the `chat` tool:

Input:
  prompt: "Write a hello world program in Rust"
  model: "gpt-4o"

The server will:
1. Compute the cost (~$0.001 for a hello-world response)
2. Sign an x402 transaction with your wallet key
3. Submit the signed payment to the gateway
4. Stream the completion back

Cost breakdown is shown in the response.
```

### Documentation

Full setup guide: https://docs.solvela.ai/en/mcp  
GitHub: https://github.com/solveladev/solvela  
Blog: https://solvela.ai/blog

---

**Note:** DRAFT SUBMISSION. Actual PR to `cursor.directory` awaits user approval.
```

---

## Screenshots Needed

**Note:** Screenshots are NOT generated in this draft. User/design team should provide:

1. **Installation flow screenshot** (800x600 minimum)
   - Terminal showing `solvela mcp install --host=cursor`
   - `.cursor/mcp.json` with env vars populated

2. **First chat example** (1200x800)
   - Cursor interface showing a `chat` tool invocation
   - Response showing cost breakdown and LLM output

3. **Model list screenshot** (800x600)
   - Output of `list_models` tool showing available providers and pricing

4. **Wallet status** (800x600)
   - `wallet_status` tool output showing balance and session spending

---

## Submission Checklist

- [ ] Research current `cursor.directory` schema (check `servers.json` structure on upstream repo)
- [ ] Confirm deeplink format matches Cursor's expected `cursor://` scheme
- [ ] Generate screenshots (dimensions noted above)
- [ ] Fork https://github.com/pontusab/cursor-directory
- [ ] Add Solvela entry to registry with all required metadata
- [ ] Create PR with this body
- [ ] Wait for maintainer approval
- [ ] Merge and verify listing appears on cursor.directory
- [ ] Update `solvela.ai/docs` with "Add to Cursor" button linking the deeplink
