# @solvela/mcp-server

MCP (Model Context Protocol) server for Solvela -- lets Claude Code, Claude Desktop, and any MCP-compatible host pay for LLM calls with USDC on Solana transparently.

MCP is an open protocol that allows AI assistants to use external tools. This server exposes the Solvela gateway as a set of MCP tools: chat with any LLM model, use smart routing, check wallet status, list models, and track spending -- all with automatic x402 payment handling.

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
| `SOLANA_WALLET_KEY` | required (when signing enabled) | Base58-encoded Solana keypair secret key |
| `SOLANA_RPC_URL` | required (when signing enabled) | Solana RPC endpoint (e.g. `https://api.mainnet-beta.solana.com`) |
| `SOLANA_WALLET_ADDRESS` | not configured | Wallet pubkey shown in `wallet_status` and `spending` |

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

No parameters.

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

- The `SOLANA_WALLET_KEY` never appears in logs, error messages, stack traces, or tool responses.
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
