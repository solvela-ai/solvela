// ─── Gateway API types ────────────────────────────────────────────────────────

export interface ModelPricing {
  input_per_million_usdc: number;
  output_per_million_usdc: number;
  platform_fee_percent: number;
  currency: string;
}

export interface ModelCapabilities {
  streaming: boolean;
  tools: boolean;
  vision: boolean;
  reasoning: boolean;
  context_window: number;
}

export interface ExampleCost {
  input_tokens: number;
  output_tokens: number;
  provider_cost_usdc: string;
  platform_fee_usdc: string;
  total_usdc: string;
}

export interface Model {
  id: string;
  display_name: string;
  provider: string;
  pricing: ModelPricing;
  capabilities: ModelCapabilities;
  example_1k_token_request: ExampleCost;
}

export interface PricingResponse {
  platform: {
    name: string;
    chain: string;
    token: string;
    usdc_mint: string;
    fee_percent: number;
    settlement: string;
  };
  models: Model[];
}

export interface HealthResponse {
  status: "ok" | "degraded" | "down";
  version: string;
}

// ─── Dashboard UI types ───────────────────────────────────────────────────────

export interface SpendDataPoint {
  date: string;
  spend: number;
  requests: number;
}

export interface ModelUsage {
  model: string;
  provider: string;
  requests: number;
  spend: number;
  pct: number;
}

export interface WalletTx {
  signature: string;
  model: string;
  amount: string;
  timestamp: string;
  status: "confirmed" | "pending" | "failed";
}

export interface DashboardStats {
  totalSpend: number;
  totalRequests: number;
  avgCostPerRequest: number;
  savingsVsOpenAI: number;
  activeModels: number;
  walletBalance: number;
}
