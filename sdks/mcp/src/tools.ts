/**
 * MCP tool definitions for RustyClawRouter.
 *
 * Each tool maps directly to a capability of the gateway:
 *   chat          — send a prompt to any model
 *   smart_chat    — auto-routed chat (eco/auto/premium/free profile)
 *   wallet_status — show USDC balance and gateway connectivity
 *   list_models   — available models with pricing
 *   spending      — session spend summary and budget status
 */

import type { Tool } from '@modelcontextprotocol/sdk/types.js';

export const TOOLS: Tool[] = [
  {
    name: 'chat',
    description:
      'Send a prompt to a specific LLM model through the RustyClawRouter gateway. ' +
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
      'List all LLM models available through the RustyClawRouter gateway, ' +
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
      'and the wallet address being used.',
    inputSchema: {
      type: 'object',
      properties: {},
      required: [],
    },
  },
];
