<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# src

## Purpose
MCP server implementation. Three files: entry point + server wiring, Solvela client, and tool definitions.

## Key Files
| File | Description |
|------|-------------|
| `index.ts` | Entry point — boots the MCP server, registers tools, connects to stdio transport |
| `client.ts` | Solvela gateway client — wraps HTTP calls, handles x402 flow |
| `tools.ts` | Tool definitions — name, input schema, handler for each MCP tool |

## Subdirectories
_(none)_

## For AI Agents

### Working In This Directory
- Every new tool goes in `tools.ts` with a matching input schema — tool-calling LLMs rely on schemas being tight and accurate.
- Don't accept private keys from tool arguments — read them from env once at server startup.
- Tool handler errors must be converted to MCP error responses (not thrown) so the client sees them as tool failures rather than server crashes.

### Testing Requirements
```bash
npm --prefix sdks/mcp test
```

### Common Patterns
- Stateless tool handlers — each call is independent.
- Re-use a single `Client` instance across tool calls.

## Dependencies

### Internal
_(none — leaf)_

### External
- MCP SDK, fetch API, crypto libraries.

<!-- MANUAL: -->
