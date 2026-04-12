#!/usr/bin/env node
/**
 * Solvela MCP Server
 *
 * Exposes Solvela gateway capabilities as MCP tools so that
 * Claude Code, OpenClaw agents, and any MCP-compatible host can pay for
 * LLM calls with USDC on Solana transparently.
 *
 * Usage:
 *   npx @solvela/mcp-server
 *
 * Environment variables:
 *   SOLVELA_API_URL        Gateway URL (default: https://api.solvela.ai)
 *   SOLVELA_SESSION_BUDGET Max USDC to spend this session (e.g. "1.00")
 *   SOLVELA_TIMEOUT_MS     Request timeout in ms (default: 60000)
 *   SOLANA_WALLET_ADDRESS  Wallet pubkey shown in wallet_status / spending
 */

import { Server } from '@modelcontextprotocol/sdk/server/index.js';
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js';
import {
  CallToolRequestSchema,
  ListToolsRequestSchema,
  ErrorCode,
  McpError,
} from '@modelcontextprotocol/sdk/types.js';

import { GatewayClient, type ChatMessage } from './client.js';
import { TOOLS } from './tools.js';

// ---------------------------------------------------------------------------
// Bootstrap client from environment
// ---------------------------------------------------------------------------

const budgetStr = process.env['RCR_SESSION_BUDGET'];
const client = new GatewayClient({
  apiUrl: process.env['RCR_API_URL'],
  sessionBudget: budgetStr !== undefined ? parseFloat(budgetStr) : undefined,
  timeoutMs: process.env['RCR_TIMEOUT_MS'] !== undefined
    ? parseInt(process.env['RCR_TIMEOUT_MS'], 10)
    : undefined,
});

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

const server = new Server(
  {
    name: 'solvela',
    version: '0.1.0',
  },
  {
    capabilities: { tools: {} },
  },
);

// ---- list tools -----------------------------------------------------------

server.setRequestHandler(ListToolsRequestSchema, async () => ({
  tools: TOOLS,
}));

// ---- call tool ------------------------------------------------------------

server.setRequestHandler(CallToolRequestSchema, async (request) => {
  const { name, arguments: args } = request.params;

  try {
    switch (name) {
      case 'chat': {
        const { model, prompt, system, max_tokens, temperature } = args as {
          model: string;
          prompt: string;
          system?: string;
          max_tokens?: number;
          temperature?: number;
        };

        const messages: ChatMessage[] = [];
        if (system) messages.push({ role: 'system', content: system });
        messages.push({ role: 'user', content: prompt });

        const response = await client.chat(model, messages, { maxTokens: max_tokens, temperature });
        const reply = response.choices[0]?.message.content ?? '';

        return {
          content: [
            {
              type: 'text',
              text: reply,
            },
            {
              type: 'text',
              text: formatUsage(response),
            },
          ],
        };
      }

      case 'smart_chat': {
        const { prompt, profile = 'auto', system, max_tokens } = args as {
          prompt: string;
          profile?: string;
          system?: string;
          max_tokens?: number;
        };

        const messages: ChatMessage[] = [];
        if (system) messages.push({ role: 'system', content: system });
        messages.push({ role: 'user', content: prompt });

        const response = await client.chat(profile, messages, { maxTokens: max_tokens });
        const reply = response.choices[0]?.message.content ?? '';

        return {
          content: [
            { type: 'text', text: reply },
            { type: 'text', text: formatUsage(response) },
          ],
        };
      }

      case 'wallet_status': {
        const health = await client.health();
        const walletAddress = process.env['SOLANA_WALLET_ADDRESS'] ?? 'not configured';
        const spend = client.spendSummary();

        const lines = [
          `Gateway:        ${client.apiUrl}`,
          `Status:         ${health.status}`,
          health.solana_rpc ? `Solana RPC:     ${health.solana_rpc}` : null,
          `Wallet:         ${walletAddress}`,
          `Session spent:  ${spend.session_usdc_spent} USDC`,
          spend.budget_remaining !== null
            ? `Budget left:    ${spend.budget_remaining} USDC`
            : null,
        ]
          .filter(Boolean)
          .join('\n');

        return { content: [{ type: 'text', text: lines }] };
      }

      case 'list_models': {
        const { filter } = args as { filter?: string };
        const modelsResp = await client.listModels();

        let models = modelsResp.data;
        if (filter) {
          const lower = filter.toLowerCase();
          models = models.filter((m) => m.id.toLowerCase().includes(lower));
        }

        if (models.length === 0) {
          return {
            content: [{ type: 'text', text: `No models found matching "${filter}".` }],
          };
        }

        const rows = models.map((m) => {
          const inputPrice = m.usdc_price_per_million_input
            ? `$${m.usdc_price_per_million_input}/M in`
            : '';
          const outputPrice = m.usdc_price_per_million_output
            ? `$${m.usdc_price_per_million_output}/M out`
            : '';
          const pricing = [inputPrice, outputPrice].filter(Boolean).join(', ');
          return `  ${m.id.padEnd(45)} ${pricing || '(see gateway)'}`;
        });

        const text = [`Available models (${models.length}):`, ...rows].join('\n');
        return { content: [{ type: 'text', text }] };
      }

      case 'spending': {
        const spend = client.spendSummary();

        const lines = [
          `Wallet:          ${spend.wallet_address ?? 'not configured'}`,
          `Requests:        ${spend.total_requests}`,
          `Session spent:   ${spend.session_usdc_spent} USDC`,
          spend.budget_remaining !== null
            ? `Budget remaining: ${spend.budget_remaining} USDC`
            : 'Budget:          unlimited',
        ];

        return { content: [{ type: 'text', text: lines.join('\n') }] };
      }

      default:
        throw new McpError(ErrorCode.MethodNotFound, `Unknown tool: ${name}`);
    }
  } catch (err) {
    if (err instanceof McpError) throw err;

    const message = err instanceof Error ? err.message : String(err);
    throw new McpError(ErrorCode.InternalError, message);
  }
});

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

function formatUsage(response: { model: string; usage?: { prompt_tokens: number; completion_tokens: number; total_tokens: number } }): string {
  const u = response.usage;
  if (!u) return `Model: ${response.model}`;
  return (
    `Model: ${response.model} | ` +
    `Tokens: ${u.prompt_tokens} in / ${u.completion_tokens} out / ${u.total_tokens} total`
  );
}

// ---------------------------------------------------------------------------
// Start server
// ---------------------------------------------------------------------------

async function main() {
  const transport = new StdioServerTransport();
  await server.connect(transport);
  // Server runs until the host closes the connection
}

main().catch((err) => {
  process.stderr.write(`Fatal: ${err instanceof Error ? err.message : String(err)}\n`);
  process.exit(1);
});
