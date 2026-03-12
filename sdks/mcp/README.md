# @rustyclawrouter/mcp

MCP (Model Context Protocol) server for RustyClawRouter -- lets Claude Code, Claude Desktop, and any MCP-compatible host pay for LLM calls with USDC on Solana transparently.

MCP is an open protocol that allows AI assistants to use external tools. This server exposes the RustyClawRouter gateway as a set of MCP tools: chat with any LLM model, use smart routing, check wallet status, list models, and track spending -- all with automatic x402 payment handling.

## Installation

```bash
npm install -g @rustyclawrouter/mcp
```

Or run directly:

```bash
npx @rustyclawrouter/mcp
```

## Setup with Claude Code

Add to your Claude Code MCP configuration (`.claude/settings.json` or project-level):

```json
{
  "mcpServers": {
    "rustyclawrouter": {
      "command": "npx",
      "args": ["@rustyclawrouter/mcp"],
      "env": {
        "RCR_API_URL": "http://localhost:8402",
        "RCR_SESSION_BUDGET": "1.00",
        "SOLANA_WALLET_ADDRESS": "YOUR_WALLET_PUBKEY"
      }
    }
  }
}
```

## Setup with Claude Desktop

Add to your Claude Desktop config (`claude_desktop_config.json`):

```json
{
  "mcpServers": {
    "rustyclawrouter": {
      "command": "npx",
      "args": ["@rustyclawrouter/mcp"],
      "env": {
        "RCR_API_URL": "http://localhost:8402",
        "RCR_SESSION_BUDGET": "1.00",
        "SOLANA_WALLET_ADDRESS": "YOUR_WALLET_PUBKEY"
      }
    }
  }
}
```

## Configuration

All configuration is via environment variables:

| Variable | Default | Description |
|----------|---------|-------------|
| `RCR_API_URL` | `https://api.rustyclawrouter.com` | Gateway URL |
| `RCR_SESSION_BUDGET` | unlimited | Max USDC to spend this session (e.g. `"1.00"`) |
| `RCR_TIMEOUT_MS` | `60000` | Request timeout in milliseconds |
| `SOLANA_WALLET_ADDRESS` | not configured | Wallet pubkey shown in `wallet_status` and `spending` |

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

The MCP server ships its own lightweight gateway client (`GatewayClient`) rather than depending on the TypeScript SDK. This keeps it self-contained as a standalone package.

The server communicates over stdio using the `@modelcontextprotocol/sdk` library. It:

1. Accepts tool calls from the MCP host (Claude Code, Claude Desktop, etc.)
2. Translates them into HTTP requests to the RustyClawRouter gateway
3. Handles the x402 payment flow (402 -> build payment header -> retry)
4. Tracks session spending and budget enforcement
5. Returns results as MCP tool responses

## Testing

```bash
npm test
```

Tests use Node.js built-in test runner with fetch mocking. No live gateway required:

```bash
node --test tests/server.test.ts
```

## License

MIT
