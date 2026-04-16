<!-- Parent: ../AGENTS.md -->
<!-- Generated: 2026-04-16 | Updated: 2026-04-16 -->

# mcp

## Purpose
Model Context Protocol (MCP) server that exposes Solvela as a set of tools to MCP clients (Claude Desktop, Claude Code, etc.). Consumers install the MCP server once, wire a wallet, and then any MCP-capable LLM can call Solvela through the standard tool interface.

## Key Files
| File | Description |
|------|-------------|
| `README.md` | Install + connect (MCP config JSON snippets) |
| `package.json` | NPM manifest |
| `package-lock.json` | Pinned dependency tree |
| `tsconfig.json` | TypeScript compiler config |

## Subdirectories
| Directory | Purpose |
|-----------|---------|
| `src/` | MCP server source (see `src/AGENTS.md`) |
| `tests/` | Tests (see `tests/AGENTS.md`) |
| `dist/` | Compiled output (not checked in) |
| `node_modules/` | Installed deps (not checked in) |

## For AI Agents

### Working In This Directory
- Follow the MCP SDK contract for tool registration (name, description, input schema).
- The server is a long-lived process — keep state minimal, and don't leak resources between tool calls.
- Private keys for the Solvela wallet come from the MCP server's env, not from the tool arguments (which are model-controlled).

### Testing Requirements
```bash
npm --prefix sdks/mcp test
```

### Common Patterns
- Tools named after Solvela capabilities: `chat`, `list_models`, `get_pricing`, etc.
- Input schemas validated with `zod` or the MCP SDK's built-in schema utility.

## Dependencies

### Internal
- Solvela gateway HTTP contract.

### External
- MCP SDK (`@modelcontextprotocol/sdk` or similar — see `package.json`).

<!-- MANUAL: -->
