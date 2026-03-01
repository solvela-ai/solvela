#!/usr/bin/env node
"use strict";
/**
 * RustyClawRouter MCP Server
 *
 * Exposes RustyClawRouter gateway capabilities as MCP tools so that
 * Claude Code, OpenClaw agents, and any MCP-compatible host can pay for
 * LLM calls with USDC on Solana transparently.
 *
 * Usage:
 *   npx @rustyclawrouter/mcp
 *
 * Environment variables:
 *   RCR_API_URL            Gateway URL (default: https://api.rustyclawrouter.com)
 *   RCR_SESSION_BUDGET     Max USDC to spend this session (e.g. "1.00")
 *   RCR_TIMEOUT_MS         Request timeout in ms (default: 60000)
 *   SOLANA_WALLET_ADDRESS  Wallet pubkey shown in wallet_status / spending
 */
Object.defineProperty(exports, "__esModule", { value: true });
const index_js_1 = require("@modelcontextprotocol/sdk/server/index.js");
const stdio_js_1 = require("@modelcontextprotocol/sdk/server/stdio.js");
const types_js_1 = require("@modelcontextprotocol/sdk/types.js");
const client_js_1 = require("./client.js");
const tools_js_1 = require("./tools.js");
// ---------------------------------------------------------------------------
// Bootstrap client from environment
// ---------------------------------------------------------------------------
const budgetStr = process.env['RCR_SESSION_BUDGET'];
const client = new client_js_1.GatewayClient({
    apiUrl: process.env['RCR_API_URL'],
    sessionBudget: budgetStr !== undefined ? parseFloat(budgetStr) : undefined,
    timeoutMs: process.env['RCR_TIMEOUT_MS'] !== undefined
        ? parseInt(process.env['RCR_TIMEOUT_MS'], 10)
        : undefined,
});
// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------
const server = new index_js_1.Server({
    name: 'rustyclawrouter',
    version: '0.1.0',
}, {
    capabilities: { tools: {} },
});
// ---- list tools -----------------------------------------------------------
server.setRequestHandler(types_js_1.ListToolsRequestSchema, async () => ({
    tools: tools_js_1.TOOLS,
}));
// ---- call tool ------------------------------------------------------------
server.setRequestHandler(types_js_1.CallToolRequestSchema, async (request) => {
    const { name, arguments: args } = request.params;
    try {
        switch (name) {
            case 'chat': {
                const { model, prompt, system, max_tokens, temperature } = args;
                const messages = [];
                if (system)
                    messages.push({ role: 'system', content: system });
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
                const { prompt, profile = 'auto', system, max_tokens } = args;
                const messages = [];
                if (system)
                    messages.push({ role: 'system', content: system });
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
                const { filter } = args;
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
                throw new types_js_1.McpError(types_js_1.ErrorCode.MethodNotFound, `Unknown tool: ${name}`);
        }
    }
    catch (err) {
        if (err instanceof types_js_1.McpError)
            throw err;
        const message = err instanceof Error ? err.message : String(err);
        throw new types_js_1.McpError(types_js_1.ErrorCode.InternalError, message);
    }
});
// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------
function formatUsage(response) {
    const u = response.usage;
    if (!u)
        return `Model: ${response.model}`;
    return (`Model: ${response.model} | ` +
        `Tokens: ${u.prompt_tokens} in / ${u.completion_tokens} out / ${u.total_tokens} total`);
}
// ---------------------------------------------------------------------------
// Start server
// ---------------------------------------------------------------------------
async function main() {
    const transport = new stdio_js_1.StdioServerTransport();
    await server.connect(transport);
    // Server runs until the host closes the connection
}
main().catch((err) => {
    process.stderr.write(`Fatal: ${err instanceof Error ? err.message : String(err)}\n`);
    process.exit(1);
});
//# sourceMappingURL=index.js.map