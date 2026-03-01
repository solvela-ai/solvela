/**
 * Minimal RustyClawRouter gateway client for the MCP server.
 *
 * Handles the x402 payment flow (402 → build payment header → retry)
 * without requiring the full TypeScript SDK as a dependency. The MCP
 * server is a standalone package that ships its own thin client.
 */
export interface ChatMessage {
    role: 'system' | 'user' | 'assistant';
    content: string;
}
export interface ChatChoice {
    index: number;
    message: ChatMessage;
    finish_reason: string | null;
}
export interface Usage {
    prompt_tokens: number;
    completion_tokens: number;
    total_tokens: number;
}
export interface ChatResponse {
    id: string;
    object: string;
    created: number;
    model: string;
    choices: ChatChoice[];
    usage?: Usage;
}
export interface ModelInfo {
    id: string;
    object: string;
    owned_by: string;
    usdc_price_per_million_input?: string;
    usdc_price_per_million_output?: string;
}
export interface ModelsResponse {
    object: string;
    data: ModelInfo[];
}
export interface HealthResponse {
    status: string;
    version?: string;
    solana_rpc?: string;
}
export interface CostBreakdown {
    provider_cost: string;
    platform_fee: string;
    total: string;
    currency: string;
    fee_percent: number;
}
export interface PaymentAccept {
    scheme: string;
    network: string;
    amount: string;
    asset: string;
    pay_to: string;
    max_timeout_seconds: number;
}
export interface PaymentRequired {
    x402_version: number;
    accepts: PaymentAccept[];
    cost_breakdown: CostBreakdown;
    error: string;
}
export interface SpendSummary {
    wallet_address: string | null;
    total_requests: number;
    total_usdc_spent: string;
    session_usdc_spent: string;
    budget_remaining: string | null;
}
export interface GatewayClientOptions {
    /** Gateway URL. Defaults to RCR_API_URL env var or https://api.rustyclawrouter.com */
    apiUrl?: string;
    /** Session spend budget in USDC. Requests are rejected if this would be exceeded. */
    sessionBudget?: number;
    /** Request timeout in ms. Defaults to 60000. */
    timeoutMs?: number;
}
/**
 * Lightweight gateway client used by the MCP server.
 *
 * Tracks session spend and exposes spend summary for the `spending` tool.
 * Payment headers use a stub transaction — real Solana signing would require
 * the agent to supply a pre-signed tx, which is not yet part of the MCP flow.
 */
export declare class GatewayClient {
    readonly apiUrl: string;
    private readonly sessionBudget?;
    private readonly timeoutMs;
    private sessionSpent;
    private requestCount;
    constructor(opts?: GatewayClientOptions);
    chat(model: string, messages: ChatMessage[], opts?: {
        maxTokens?: number;
        temperature?: number;
    }): Promise<ChatResponse>;
    listModels(): Promise<ModelsResponse>;
    health(): Promise<HealthResponse>;
    spendSummary(): SpendSummary;
    private fetchWithTimeout;
    private parse402;
}
//# sourceMappingURL=client.d.ts.map