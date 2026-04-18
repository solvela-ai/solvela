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
 *   SOLVELA_API_URL          Gateway URL (default: https://api.solvela.ai)
 *   SOLVELA_SESSION_BUDGET   Max USDC to spend this session (e.g. "1.00")
 *   SOLVELA_TIMEOUT_MS       Request timeout in ms (default: 60000)
 *   SOLVELA_SIGNING_MODE     auto | escrow | direct | off (default: auto)
 *   SOLVELA_ALLOW_DEV_BYPASS Set to "1" to silence dev_bypass_payment warning
 *   SOLANA_WALLET_KEY        Base58 secret key (required unless SOLVELA_SIGNING_MODE=off)
 *   SOLANA_RPC_URL           Solana RPC endpoint (required unless SOLVELA_SIGNING_MODE=off)
 *   SOLANA_WALLET_ADDRESS    Wallet pubkey shown in wallet_status / spending
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

// HF10: Validate SOLVELA_SIGNING_MODE before use.
const rawSigningMode = process.env['SOLVELA_SIGNING_MODE'] ?? 'auto';
if (!['auto', 'escrow', 'direct', 'off'].includes(rawSigningMode)) {
  process.stderr.write(
    `[solvela-mcp] Fatal: invalid SOLVELA_SIGNING_MODE='${rawSigningMode}'. Must be one of auto|escrow|direct|off.\n`,
  );
  process.exit(1);
}
const signingMode = rawSigningMode as 'auto' | 'escrow' | 'direct' | 'off';

// HF11: Validate SOLVELA_SESSION_BUDGET — reject NaN/non-positive values.
const budgetStr = process.env['SOLVELA_SESSION_BUDGET'] ?? process.env['RCR_SESSION_BUDGET']; // compat
let sessionBudget: number | undefined;
if (budgetStr !== undefined) {
  const parsed = parseFloat(budgetStr);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_SESSION_BUDGET='${budgetStr}' is not a positive number.\n`,
    );
    process.exit(1);
  }
  sessionBudget = parsed;
}

// HF11: Validate SOLVELA_TIMEOUT_MS — reject NaN/non-positive values.
const timeoutStr = process.env['SOLVELA_TIMEOUT_MS'] ?? process.env['RCR_TIMEOUT_MS']; // compat
let timeoutMs: number | undefined;
if (timeoutStr !== undefined) {
  const parsed = parseInt(timeoutStr, 10);
  if (!Number.isFinite(parsed) || parsed <= 0) {
    process.stderr.write(
      `[solvela-mcp] Fatal: SOLVELA_TIMEOUT_MS='${timeoutStr}' is not a positive integer.\n`,
    );
    process.exit(1);
  }
  timeoutMs = parsed;
}

const client = new GatewayClient({
  apiUrl: process.env['SOLVELA_API_URL'] ?? process.env['RCR_API_URL'], // compat
  sessionBudget,
  timeoutMs,
  signingMode,
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
  // T1-D / T1-E: Startup validation — fail fast when signing is enabled without required keys.
  if (signingMode !== 'off') {
    if (!process.env['SOLANA_WALLET_KEY']) {
      process.stderr.write(
        'Fatal: SOLANA_WALLET_KEY is required when signing is enabled. Set SOLVELA_SIGNING_MODE=off to run without signing.\n',
      );
      process.exit(1);
    }
    if (!process.env['SOLANA_RPC_URL']) {
      process.stderr.write(
        'Fatal: SOLANA_RPC_URL is required when signing is enabled. Set SOLVELA_SIGNING_MODE=off to run without signing.\n',
      );
      process.exit(1);
    }
  }

  // HF6: Log resolved gateway URL and signing mode at startup — makes typos visible.
  process.stderr.write(
    `[solvela-mcp] gateway=${client.apiUrl} signingMode=${signingMode}\n`,
  );

  // HF7: Health check with short timeout (5 s) so it never blocks MCP handshake.
  const healthTimeoutMs = 5000;
  try {
    const healthController = new AbortController();
    const healthTimer = setTimeout(() => healthController.abort(), healthTimeoutMs);
    let health: Record<string, unknown>;
    try {
      const healthResp = await Promise.race([
        client.health(),
        new Promise<never>((_, reject) =>
          setTimeout(() => reject(new Error(`Health check timed out after ${healthTimeoutMs}ms`)), healthTimeoutMs),
        ),
      ]);
      health = healthResp as Record<string, unknown>;
    } finally {
      clearTimeout(healthTimer);
    }

    // HF12: Stronger warning when signing is ON but dev_bypass is active —
    // key is in memory but payments are not verified.
    if (health['dev_bypass_payment'] === true && process.env['SOLVELA_ALLOW_DEV_BYPASS'] !== '1') {
      if (signingMode !== 'off') {
        process.stderr.write(
          `[solvela-mcp] WARN: Gateway is in dev_bypass_payment mode but signingMode=${signingMode} — ` +
          `your wallet key is loaded in memory but the gateway is NOT verifying payments. ` +
          `Consider SOLVELA_SIGNING_MODE=off for dev. Set SOLVELA_ALLOW_DEV_BYPASS=1 to silence.\n`,
        );
      } else {
        process.stderr.write(
          '[solvela-mcp] WARN: Gateway is running in dev_bypass_payment mode. Payments will NOT be verified.' +
          ' Set SOLVELA_ALLOW_DEV_BYPASS=1 to silence this warning.\n',
        );
      }
    }
  } catch (err) {
    // HF7: Gateway unreachable or timed out — warn but do not prevent server from starting.
    const msg = err instanceof Error ? err.message : String(err);
    process.stderr.write(
      `[solvela-mcp] WARN: Gateway health check failed (${msg}). Gateway URL: ${client.apiUrl}. The server will start but chat calls may fail.\n`,
    );
  }

  const transport = new StdioServerTransport();
  await server.connect(transport);
  // Server runs until the host closes the connection
}

main().catch((err) => {
  process.stderr.write(`Fatal: ${err instanceof Error ? err.message : String(err)}\n`);
  process.exit(1);
});
