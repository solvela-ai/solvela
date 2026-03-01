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
export declare const TOOLS: Tool[];
//# sourceMappingURL=tools.d.ts.map