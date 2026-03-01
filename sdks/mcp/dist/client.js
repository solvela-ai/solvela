"use strict";
/**
 * Minimal RustyClawRouter gateway client for the MCP server.
 *
 * Handles the x402 payment flow (402 → build payment header → retry)
 * without requiring the full TypeScript SDK as a dependency. The MCP
 * server is a standalone package that ships its own thin client.
 */
Object.defineProperty(exports, "__esModule", { value: true });
exports.GatewayClient = void 0;
/**
 * Lightweight gateway client used by the MCP server.
 *
 * Tracks session spend and exposes spend summary for the `spending` tool.
 * Payment headers use a stub transaction — real Solana signing would require
 * the agent to supply a pre-signed tx, which is not yet part of the MCP flow.
 */
class GatewayClient {
    apiUrl;
    sessionBudget;
    timeoutMs;
    sessionSpent = 0;
    requestCount = 0;
    constructor(opts = {}) {
        this.apiUrl = (opts.apiUrl ?? process.env['RCR_API_URL'] ?? 'https://api.rustyclawrouter.com').replace(/\/$/, '');
        this.sessionBudget = opts.sessionBudget;
        this.timeoutMs = opts.timeoutMs ?? 60_000;
    }
    // ---------------------------------------------------------------------------
    // Public API
    // ---------------------------------------------------------------------------
    async chat(model, messages, opts = {}) {
        const body = {
            model,
            messages,
            max_tokens: opts.maxTokens,
            temperature: opts.temperature,
            stream: false,
        };
        const url = `${this.apiUrl}/v1/chat/completions`;
        let resp = await this.fetchWithTimeout(url, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(body),
        });
        if (resp.status === 402) {
            const paymentInfo = await this.parse402(resp);
            if (!paymentInfo)
                throw new Error('Failed to parse 402 response from gateway');
            const cost = parseFloat(paymentInfo.cost_breakdown.total);
            if (this.sessionBudget !== undefined && this.sessionSpent + cost > this.sessionBudget) {
                throw new Error(`Session budget $${this.sessionBudget.toFixed(6)} USDC exceeded ` +
                    `(spent: $${this.sessionSpent.toFixed(6)}, request cost: $${cost.toFixed(6)})`);
            }
            const paymentHeader = buildPaymentHeader(paymentInfo, url);
            resp = await this.fetchWithTimeout(url, {
                method: 'POST',
                headers: {
                    'content-type': 'application/json',
                    'payment-signature': paymentHeader,
                },
                body: JSON.stringify(body),
            });
            this.sessionSpent += cost;
            this.requestCount += 1;
        }
        else if (resp.ok) {
            this.requestCount += 1;
        }
        if (!resp.ok) {
            const text = await resp.text().catch(() => '');
            throw new Error(`Gateway error ${resp.status}: ${text}`);
        }
        return resp.json();
    }
    async listModels() {
        const resp = await this.fetchWithTimeout(`${this.apiUrl}/v1/models`);
        if (!resp.ok)
            throw new Error(`Failed to list models: ${resp.status}`);
        return resp.json();
    }
    async health() {
        const resp = await this.fetchWithTimeout(`${this.apiUrl}/health`);
        if (!resp.ok)
            throw new Error(`Health check failed: ${resp.status}`);
        return resp.json();
    }
    spendSummary() {
        return {
            wallet_address: process.env['SOLANA_WALLET_ADDRESS'] ?? null,
            total_requests: this.requestCount,
            total_usdc_spent: this.sessionSpent.toFixed(6),
            session_usdc_spent: this.sessionSpent.toFixed(6),
            budget_remaining: this.sessionBudget !== undefined
                ? Math.max(0, this.sessionBudget - this.sessionSpent).toFixed(6)
                : null,
        };
    }
    // ---------------------------------------------------------------------------
    // Internals
    // ---------------------------------------------------------------------------
    async fetchWithTimeout(url, init) {
        const controller = new AbortController();
        const id = setTimeout(() => controller.abort(), this.timeoutMs);
        try {
            return await fetch(url, { ...init, signal: controller.signal });
        }
        finally {
            clearTimeout(id);
        }
    }
    async parse402(resp) {
        try {
            // eslint-disable-next-line @typescript-eslint/no-explicit-any
            const body = await resp.json();
            const msg = body?.error?.message;
            if (typeof msg === 'string')
                return JSON.parse(msg);
            if (body?.x402_version && body?.accepts)
                return body;
            return null;
        }
        catch {
            return null;
        }
    }
}
exports.GatewayClient = GatewayClient;
/**
 * Build a base64-encoded `PAYMENT-SIGNATURE` header from a 402 response.
 * Uses a stub transaction — real Solana signing is out of scope for the MCP server.
 */
function buildPaymentHeader(paymentInfo, resourceUrl) {
    const accept = paymentInfo.accepts[0];
    if (!accept)
        throw new Error('No payment accepts in 402 response');
    const payload = {
        x402_version: 2,
        resource: { url: resourceUrl, method: 'POST' },
        accepted: accept,
        payload: { transaction: 'STUB_BASE64_TX' },
    };
    return Buffer.from(JSON.stringify(payload), 'utf-8').toString('base64');
}
//# sourceMappingURL=client.js.map