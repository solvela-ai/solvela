# @solvela/mcp-server

MCP (Model Context Protocol) server for Solvela -- lets Claude Code, Claude Desktop, and any MCP-compatible host pay for LLM calls with USDC on Solana transparently.

MCP is an open protocol that allows AI assistants to use external tools. This server exposes the Solvela gateway as a set of MCP tools: chat with any LLM model, use smart routing, check wallet status, list models, and track spending -- all with automatic x402 payment handling.

## Quickstart

Install the `solvela` CLI, then run the one-line installer for your host:

```bash
# Install the Solvela CLI (once)
cargo install --path crates/cli

# Install into your preferred host
solvela mcp install --host=claude-code
solvela mcp install --host=cursor
solvela mcp install --host=claude-desktop
solvela mcp install --host=openclaw
```

The installer writes the correct config for your host and prints a reminder to
set `SOLANA_WALLET_KEY` in your shell environment (it is intentionally never
written to disk by default):

```bash
export SOLANA_WALLET_KEY=<your-base58-keypair>
```

Options:

```
--scope=user|project    user-scoped (default) or project-scoped config
--wallet=<pubkey>       wallet address to embed (defaults to ~/.solvela/wallet.json)
--budget=<usdc>         set SOLVELA_SESSION_BUDGET (e.g. "2.00")
--signing-mode=auto     auto|escrow|direct|off
--dry-run               print config without writing
--diff                  show what would change vs the existing config
--force                 overwrite an existing entry without prompting
```

To remove:

```bash
solvela mcp uninstall --host=claude-code
```

The manual JSON snippets for each host are below as a fallback reference.

## Installation

```bash
npm install -g @solvela/mcp-server
```

Or run directly:

```bash
npx @solvela/mcp-server
```

## Setup with Claude Code

Add to your Claude Code MCP configuration (`.claude/settings.json` or project-level):

```json
{
  "mcpServers": {
    "solvela": {
      "command": "npx",
      "args": ["@solvela/mcp-server"],
      "env": {
        "SOLVELA_API_URL": "http://localhost:8402",
        "SOLVELA_SESSION_BUDGET": "1.00",
        "SOLANA_WALLET_KEY": "YOUR_BASE58_SECRET_KEY",
        "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com",
        "SOLANA_WALLET_ADDRESS": "YOUR_WALLET_PUBKEY"
      }
    }
  }
}
```

> **Security:** Never commit `SOLANA_WALLET_KEY` to version control. Store it in a
> `.env` file with `0600` permissions or use your OS keychain. See [Security](#security).

## Setup with Claude Desktop

Add to your Claude Desktop config (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "solvela": {
      "command": "npx",
      "args": ["@solvela/mcp-server"],
      "env": {
        "SOLVELA_API_URL": "http://localhost:8402",
        "SOLVELA_SESSION_BUDGET": "1.00",
        "SOLANA_WALLET_KEY": "YOUR_BASE58_SECRET_KEY",
        "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com",
        "SOLANA_WALLET_ADDRESS": "YOUR_WALLET_PUBKEY"
      }
    }
  }
}
```

> **Security:** Never commit `SOLANA_WALLET_KEY` to version control. Store it in a
> `.env` file with `0600` permissions or use your OS keychain. See [Security](#security).

## Setup with Cursor

Add to `.cursor/mcp.json` (project) or `~/.cursor/mcp.json` (global):

```json
{
  "mcpServers": {
    "solvela": {
      "type": "stdio",
      "command": "npx",
      "args": ["@solvela/mcp-server"],
      "env": {
        "SOLVELA_API_URL": "http://localhost:8402",
        "SOLVELA_SESSION_BUDGET": "1.00",
        "SOLANA_WALLET_KEY": "YOUR_BASE58_SECRET_KEY",
        "SOLANA_RPC_URL": "https://api.mainnet-beta.solana.com",
        "SOLANA_WALLET_ADDRESS": "YOUR_WALLET_PUBKEY"
      }
    }
  }
}
```

> **Security:** Never commit `SOLANA_WALLET_KEY` to version control. Store it in a
> `.env` file with `0600` permissions or use your OS keychain. See [Security](#security).

## Configuration

All configuration is via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `SOLVELA_API_URL` | `https://api.solvela.ai` | Gateway URL |
| `SOLVELA_SESSION_BUDGET` | unlimited | Max USDC to spend this session (e.g. `"1.00"`) |
| `SOLVELA_TIMEOUT_MS` | `60000` | Request timeout in milliseconds |
| `SOLVELA_SIGNING_MODE` | `auto` | Payment signing mode: `auto`, `escrow`, `direct`, or `off` |
| `SOLVELA_ALLOW_DEV_BYPASS` | — | Set to `1` to silence the dev_bypass_payment gateway warning |
| `SOLVELA_ESCROW_MODE` | — | Set to `enabled` to expose the `deposit_escrow` tool |
| `SOLVELA_MAX_ESCROW_DEPOSIT` | `5.0` | Per-call deposit cap in USDC (applies only when escrow mode is enabled) |
| `SOLVELA_MAX_ESCROW_SESSION` | `20.0` | Cumulative session deposit cap in USDC (applies only when escrow mode is enabled) |
| `SOLVELA_ESCROW_PROGRAM_ID` | required (when escrow enabled) | Base58 address of the Solvela escrow program on Solana |
| `SOLVELA_RECIPIENT_WALLET` | required (when escrow enabled) | Base58 wallet address that receives escrow payments |
| `SOLANA_WALLET_KEY` | required (when signing enabled) | Base58-encoded Solana keypair secret key |
| `SOLANA_RPC_URL` | required (when signing enabled) | Solana RPC endpoint (e.g. `https://api.mainnet-beta.solana.com`) |
| `SOLANA_WALLET_ADDRESS` | not configured | Wallet pubkey shown in `wallet_status` and `spending` |

### Escrow Mode

Set `SOLVELA_ESCROW_MODE=enabled` to expose the `deposit_escrow` tool. This is intended for agent-driven workloads where the agent pre-funds an escrow PDA and the gateway claims only what was actually used.

**Do not enable escrow mode for interactive chat sessions** — the per-session deposit cap is a safeguard, not a substitute for careful budget management.

When escrow mode is enabled, the server logs the effective caps at startup:

```
[solvela-mcp] escrow=enabled max-deposit=$5.00 max-session=$20.00
```

Caps are enforced in-process and persisted to `~/.solvela/mcp-session.json` so they survive restarts.

### Session Persistence

The MCP server writes `~/.solvela/mcp-session.json` on every spend event. This file tracks:

- `session_spent` — cumulative USDC spent via `chat` / `smart_chat`
- `escrow_deposits_session` — cumulative USDC deposited via `deposit_escrow`
- `request_count` — total requests this session
- `last_updated` — ISO timestamp of last write

Example file:

```json
{
  "session_spent": 0.012500,
  "escrow_deposits_session": 4.000000,
  "request_count": 5,
  "last_updated": "2026-04-18T12:34:56.789Z",
  "version": 1
}
```

The file is written atomically (via a `.tmp` rename) with `0600` permissions on Unix. If the file is missing, corrupt, or has an unknown schema version, the server resets to zero and logs a `WARN` to stderr — it does not crash.

To reset the session counters and delete the file, call the `spending` tool with `reset: true`:

```json
{ "tool": "spending", "arguments": { "reset": true } }
```

Or delete the file manually:

```bash
rm ~/.solvela/mcp-session.json
```

**Security:** The session file never contains wallet keys or signing material. It is safe to inspect and delete at any time.

### Signing Modes

- **`auto`** (default) — The SDK prefers escrow deposits when the gateway advertises them, falling back to direct TransferChecked. Recommended for production.
- **`escrow`** — Only use escrow payment schemes. Fails if the gateway does not advertise escrow.
- **`direct`** — Only use direct USDC TransferChecked payment schemes. Ignores escrow offers.
- **`off`** — Do not sign payments. Useful when the gateway runs with `dev_bypass_payment` enabled (development only). `SOLANA_WALLET_KEY` and `SOLANA_RPC_URL` are not required in this mode.

## Available Tools

The MCP server exposes five tools:

### `chat`

Send a prompt to a specific LLM model through the gateway. Payment is handled automatically via USDC on Solana.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `model` | `string` | yes | Model identifier (e.g. `openai/gpt-4o`, `anthropic/claude-sonnet-4`) |
| `prompt` | `string` | yes | The user message |
| `system` | `string` | no | System prompt to set assistant behaviour |
| `max_tokens` | `number` | no | Maximum tokens in the response |
| `temperature` | `number` | no | Sampling temperature (0.0--2.0) |

### `smart_chat`

Send a prompt using the gateway smart router. It automatically picks the cheapest capable model for the complexity of your request.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `prompt` | `string` | yes | The user message |
| `profile` | `string` | no | Routing profile: `eco`, `auto` (default), `premium`, `free` |
| `system` | `string` | no | System prompt |
| `max_tokens` | `number` | no | Maximum tokens in the response |

### `wallet_status`

Check the status of the configured Solana wallet and gateway connectivity. Returns the wallet address, gateway health, Solana RPC status, and current session spending.

No parameters.

### `list_models`

List all LLM models available through the gateway, including USDC pricing per million tokens.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `filter` | `string` | no | Substring filter (e.g. `gpt`, `claude`, `gemini`) |

### `spending`

Show USDC spending statistics for the current session: total spent, request count, remaining budget, and wallet address.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `reset` | `boolean` | no | If `true`, reset all session counters to zero and delete `~/.solvela/mcp-session.json` |

### `deposit_escrow`

Deposit USDC into a trustless escrow PDA on Solana for a future Solvela call. The gateway claims only what was actually used after the request completes; the remainder auto-refunds.

**Only visible when `SOLVELA_ESCROW_MODE=enabled`.**

Requires `SOLANA_WALLET_KEY` and `SOLANA_RPC_URL` regardless of `SOLVELA_SIGNING_MODE`.

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `amount_usdc` | `string` | yes | Amount to deposit in USDC (e.g. `"2.50"`) |
| `max_timeout_seconds` | `number` | no | Escrow expiry in seconds (default: `300`) |

Returns:

```json
{
  "deposit_tx_signature": "<base58 Solana tx signature>",
  "escrow_pda": "<base58 PDA address>",
  "amount_deposited_usdc": "2.500000",
  "session_deposits_total_usdc": "6.500000",
  "session_deposits_cap_usdc": "20.000000"
}
```

**Caps:**
- Per-call: `SOLVELA_MAX_ESCROW_DEPOSIT` (default `$5.00`). Reject amounts above this threshold.
- Session: `SOLVELA_MAX_ESCROW_SESSION` (default `$20.00`). Cumulative deposits this session, persisted across restarts.

If the on-chain deposit broadcast fails, the session cap is NOT incremented. The transaction is confirmed before returning (60 s timeout). If confirmation times out, the error message includes the transaction signature so you can check Solana Explorer.

## Examples

Once the MCP server is configured, these tools are available to the AI assistant automatically. Here is how they work in practice:

**Chat with a specific model:**

The assistant calls the `chat` tool with `model: "openai/gpt-4o"` and `prompt: "Explain x402"`. The MCP server sends the request to the gateway, handles the x402 payment flow, and returns the response along with token usage.

**Use smart routing for cost optimization:**

The assistant calls `smart_chat` with `prompt: "What is 2+2?"` and `profile: "eco"`. The gateway analyzes prompt complexity and routes to the cheapest capable model.

**Check wallet and gateway status:**

The assistant calls `wallet_status`. The server returns gateway connectivity, Solana RPC status, configured wallet address, and current session spend.

**List available models:**

The assistant calls `list_models` with `filter: "claude"`. Returns all matching models with their per-million-token USDC pricing for input and output.

**Monitor spending:**

The assistant calls `spending`. Returns total requests made, USDC spent this session, and remaining budget if one was configured.

## Architecture

The MCP server depends on `@solvela/sdk` for real on-chain USDC payment signing. When a 402 Payment Required response is received from the gateway, the server:

1. Parses the payment requirements (accepted schemes, amount, recipient)
2. Applies the configured signing mode filter to the accepted schemes
3. Calls `createPaymentHeader` from `@solvela/sdk` with the agent's private key
4. Retries the request with the `payment-signature` header

The server communicates over stdio using the `@modelcontextprotocol/sdk` library. It:

1. Accepts tool calls from the MCP host (Claude Code, Claude Desktop, etc.)
2. Translates them into HTTP requests to the Solvela gateway
3. Handles the x402 payment flow (402 -> sign -> retry)
4. Tracks session spending and budget enforcement (concurrency-safe via mutex)
5. Returns results as MCP tool responses

## Security

### Escrow mode — model-controlled money movement

When `SOLVELA_ESCROW_MODE=enabled`, the `deposit_escrow` tool becomes
available to the AI model. The model decides when to deposit and how
much (up to `SOLVELA_MAX_ESCROW_DEPOSIT` per call, `SOLVELA_MAX_ESCROW_SESSION`
per session).

**Threat model:** A prompt-injected or misaligned model could deposit
up to the session cap without your explicit approval.

**Mitigations:**
- Caps are enforced both per-call and cumulatively per session.
- The session cap is checked atomically (mutex-protected) so parallel tool
  invocations cannot exceed the limit via a race condition.
- Set caps conservatively (defaults: $5/call, $20/session).
- The `spending` tool shows cumulative deposits at any time.
- Run `spending` with `reset: true` to clear session counters.

**WARNING on address trust:** `SOLVELA_RECIPIENT_WALLET` and
`SOLVELA_ESCROW_PROGRAM_ID` control where your USDC is deposited.
Verify these match the official Solvela addresses published at
https://docs.solvela.ai/addresses before enabling. Both values are
validated as valid Solana pubkeys at server startup — an invalid or
typo'd address causes an immediate fatal error rather than a silent
misdirected deposit. An attacker who can modify your MCP config file
can still redirect deposits to a different valid address.

**Multi-process warning:** Session cap enforcement is in-process only.
Running two MCP server instances pointing at the same session file may
allow the cumulative cap to be exceeded (no cross-process file lock).
Use a single MCP server instance per session file.

### Key storage model

`SOLANA_WALLET_KEY` is a **hot-wallet secret**. Anyone who can read it can drain your USDC. Treat it with the same care as an SSH private key.

**The installer does NOT write `SOLANA_WALLET_KEY` to any config file by default.** The generated config intentionally omits the key. You must supply it through one of the secure paths below.

**`--include-key` flag (dev/CI only):** Passing this flag writes a plaintext placeholder into the config file. The installer emits a prominent stderr warning. Only use this in isolated dev environments or ephemeral CI runners where the config file is never committed or shared. The placeholder must be replaced with your actual key before the MCP server will work.

### Recommended: store the key in `~/.solvela/env`

```bash
mkdir -p ~/.solvela
echo "SOLANA_WALLET_KEY=<your-base58-private-key>" > ~/.solvela/env
chmod 0600 ~/.solvela/env
```

**Cursor users:** The installer writes `"envFile": "${userHome}/.solvela/env"` into the Cursor config by default (pass `--no-envfile` to disable). Cursor will source this file automatically, keeping the key out of the JSON config entirely. The file at `~/.solvela/env` should be `chmod 0600` and must never be committed to version control.

**Claude Code / Claude Desktop / OpenClaw users:** Set the key in your shell profile:

```bash
export SOLANA_WALLET_KEY=<your-base58-private-key>
```

Or store it in a `0600` file and source it from your profile.

### General rules

- Never commit `SOLANA_WALLET_KEY` to version control. Add `*.env`, `.solvela/env`, and any file containing the key to `.gitignore`.
- The MCP server never logs, echoes, or returns the key in tool responses. Stack traces and error messages are also sanitized.
- The SDK zeroes secret key bytes in memory after signing.
- Private key material flows only from environment variable into the signer — it is never passed through tool arguments (which are model-controlled).

## Testing

```bash
npm test
```

Tests use Node.js built-in test runner with fetch mocking. No live gateway or Solana RPC required:

```bash
node --test tests/server.test.ts
```

## License

MIT
