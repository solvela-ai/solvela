/**
 * MCP tool definitions for Solvela.
 *
 * Each tool maps directly to a capability of the gateway:
 *   chat          — send a prompt to any model
 *   smart_chat    — auto-routed chat (eco/auto/premium/free profile)
 *   wallet_status — show USDC balance and gateway connectivity
 *   list_models   — available models with pricing
 *   spending      — session spend summary and budget status
 *   deposit_escrow — deposit USDC into a trustless escrow PDA (SOLVELA_ESCROW_MODE=enabled only)
 */

import type { Tool } from '@modelcontextprotocol/sdk/types.js';

const BASE_TOOLS: Tool[] = [
  {
    name: 'chat',
    description:
      'Send a prompt to a specific LLM model through the Solvela gateway. ' +
      'Payment is handled automatically via USDC on Solana (x402 protocol). ' +
      'Use list_models to see available models and their pricing.',
    inputSchema: {
      type: 'object',
      properties: {
        model: {
          type: 'string',
          description:
            'Model identifier, e.g. "openai/gpt-4o", "anthropic/claude-sonnet-4", ' +
            '"google/gemini-2.5-flash". Use list_models to see all options.',
        },
        prompt: {
          type: 'string',
          description: 'The user message to send to the model.',
        },
        system: {
          type: 'string',
          description: 'Optional system prompt to set the assistant behaviour.',
        },
        max_tokens: {
          type: 'number',
          description: 'Maximum tokens in the response. Defaults to model maximum.',
        },
        temperature: {
          type: 'number',
          description: 'Sampling temperature 0.0–2.0. Defaults to model default.',
        },
      },
      required: ['model', 'prompt'],
    },
  },
  {
    name: 'smart_chat',
    description:
      'Send a prompt using the gateway smart router — it automatically picks the ' +
      'cheapest capable model for the complexity of your request. ' +
      'Profiles: "eco" (cheapest), "auto" (balanced, default), "premium" (best quality), ' +
      '"free" (no-cost open-source models where available).',
    inputSchema: {
      type: 'object',
      properties: {
        prompt: {
          type: 'string',
          description: 'The user message to send.',
        },
        profile: {
          type: 'string',
          enum: ['eco', 'auto', 'premium', 'free'],
          description: 'Routing profile. Defaults to "auto".',
        },
        system: {
          type: 'string',
          description: 'Optional system prompt.',
        },
        max_tokens: {
          type: 'number',
          description: 'Maximum tokens in the response.',
        },
      },
      required: ['prompt'],
    },
  },
  {
    name: 'wallet_status',
    description:
      'Check the status of the configured Solana wallet and gateway connectivity. ' +
      'Returns the wallet address (from SOLANA_WALLET_ADDRESS env var), gateway health, ' +
      'and Solana RPC status.',
    inputSchema: {
      type: 'object',
      properties: {},
      required: [],
    },
  },
  {
    name: 'list_models',
    description:
      'List all LLM models available through the Solvela gateway, ' +
      'including their USDC pricing per million tokens (input and output).',
    inputSchema: {
      type: 'object',
      properties: {
        filter: {
          type: 'string',
          description:
            'Optional substring filter to narrow results, e.g. "gpt", "claude", "gemini".',
        },
      },
      required: [],
    },
  },
  {
    name: 'spending',
    description:
      'Show USDC spending statistics for the current session: total USDC spent, ' +
      'number of requests made, remaining budget (if a budget was configured), ' +
      'and the wallet address being used. ' +
      'Pass reset: true to clear session spending counters and delete the persisted session file.',
    inputSchema: {
      type: 'object',
      properties: {
        reset: {
          type: 'boolean',
          description:
            'If true, reset all session counters to zero and delete ~/.solvela/mcp-session.json. ' +
            'Useful to start a fresh budget tracking session.',
        },
      },
      required: [],
    },
  },
];

const DEPOSIT_ESCROW_TOOL: Tool = {
  name: 'deposit_escrow',
  description:
    'Deposit USDC into a trustless escrow PDA on Solana for a future Solvela call. ' +
    'The gateway claims only what was actually used after the request completes; remainder auto-refunds. ' +
    'Requires SOLVELA_ESCROW_MODE=enabled. Per-call cap: SOLVELA_MAX_ESCROW_DEPOSIT (default $5). ' +
    'Session cap: SOLVELA_MAX_ESCROW_SESSION (default $20 cumulative).',
  inputSchema: {
    type: 'object',
    properties: {
      amount_usdc: {
        type: 'string',
        description: 'Amount to deposit in USDC (e.g. "2.50")',
      },
      max_timeout_seconds: {
        type: 'number',
        description: 'Optional escrow expiry in seconds (default 300s).',
      },
    },
    required: ['amount_usdc'],
  },
};

/**
 * Return the tool list filtered by enabled features.
 *
 * @param opts.escrowEnabled Whether the deposit_escrow tool should be included.
 *   Computed once at module-load time in index.ts and passed here so that
 *   process.env is not re-read on every ListTools request (single source of truth).
 *   Falls back to reading process.env for callers that do not pass the argument
 *   (e.g. test code that imports getTools() directly).
 */
export function getTools(opts?: { escrowEnabled?: boolean }): Tool[] {
  const escrowEnabled = opts?.escrowEnabled ?? process.env['SOLVELA_ESCROW_MODE'] === 'enabled';
  if (escrowEnabled) {
    return [...BASE_TOOLS, DEPOSIT_ESCROW_TOOL];
  }
  return BASE_TOOLS;
}

/**
 * @deprecated Use getTools() instead. Kept for backwards compatibility with existing tests
 * that import TOOLS directly. Will be removed in a future version.
 */
export const TOOLS: Tool[] = BASE_TOOLS;
