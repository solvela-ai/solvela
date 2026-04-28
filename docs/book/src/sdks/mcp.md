# MCP Server

The MCP (Model Context Protocol) server integrates Solvela directly into Claude Code and other MCP-compatible AI assistants. Agents can call LLM models, check wallet status, and manage spending through MCP tool calls.

## What Is MCP

The [Model Context Protocol](https://modelcontextprotocol.io/) is a standard for connecting AI assistants to external tools and data sources. The Solvela MCP server exposes gateway capabilities as structured tools that Claude Code can invoke during conversations.

## Installation

The MCP server is a TypeScript package in `sdks/mcp/`:

```bash
cd sdks/mcp
npm install
npm run build
```

Or run directly with npx:

```bash
npx @solvela/mcp
```

## Setup with Claude Code

Add the MCP server to your Claude Code configuration:

### Configuration File

Add to your MCP config (typically `~/.claude/mcp.json` or project-level `.claude/mcp.json`):

```json
{
  "mcpServers": {
    "solvela": {
      "command": "node",
      "args": ["/path/to/sdks/mcp/dist/index.js"],
      "env": {
        "SOLVELA_API_URL": "http://localhost:8402",
        "SOLANA_WALLET_KEY": "your-base58-private-key",
        "SOLANA_RPC_URL": "https://api.devnet.solana.com"
      }
    }
  }
}
```

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `SOLVELA_API_URL` | Yes | Gateway URL |
| `SOLANA_WALLET_KEY` | Yes | Base58 wallet private key for payments |
| `SOLANA_RPC_URL` | No | Solana RPC endpoint |

## Available Tools

### `chat`

Send a prompt to a specific LLM model through the gateway.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `model` | `string` | Yes | Model ID (e.g., `"openai/gpt-4o"`) |
| `prompt` | `string` | Yes | The user message |
| `system` | `string` | No | System prompt |
| `max_tokens` | `number` | No | Maximum response tokens |
| `temperature` | `number` | No | Sampling temperature (0.0--2.0) |

**Example usage in Claude Code:**

> "Use the chat tool to ask gpt-4o to explain Solana's consensus mechanism"

### `smart_chat`

Auto-routed chat using a routing profile. The gateway scores the request and selects the optimal model.

**Parameters:**

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `prompt` | `string` | Yes | The user message |
| `profile` | `string` | No | Routing profile: `eco`, `auto`, `premium`, `free` |
| `system` | `string` | No | System prompt |
| `max_tokens` | `number` | No | Maximum response tokens |

### `wallet_status`

Check USDC balance and gateway connectivity.

**Parameters:** None

**Returns:** Wallet address, USDC balance, gateway reachability status.

### `list_models`

List available models with pricing information.

**Parameters:** None

**Returns:** Array of models with IDs, providers, pricing, and capabilities.

### `spending`

View session spend summary and budget status.

**Parameters:** None

**Returns:** Total spent in current session, budget remaining (if set), per-model breakdown.

## Example Session

Once configured, you can use natural language with Claude Code:

> "What models are available through Solvela?"

Claude Code calls the `list_models` tool and presents the results.

> "Use the eco profile to summarize this document"

Claude Code calls `smart_chat` with `profile: "eco"` and passes the prompt.

> "Check my wallet balance"

Claude Code calls `wallet_status` and reports the USDC balance.

> "How much have I spent this session?"

Claude Code calls `spending` and shows the breakdown.

## Development

```bash
cd sdks/mcp

# Build
npm run build

# Watch mode
npm run dev

# Run tests
npm test

# Start the server
npm start
```

The MCP server uses the `@modelcontextprotocol/sdk` package and communicates over stdio.
