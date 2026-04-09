export type Role = 'system' | 'user' | 'assistant' | 'tool';

export interface ChatMessage {
  role: Role;
  content: string;
  name?: string;
}

export interface ChatRequest {
  model: string;
  messages: ChatMessage[];
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
  stream?: boolean;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export interface ChatChoice {
  index: number;
  message: ChatMessage;
  finish_reason: string | null;
}

export interface ChatResponse {
  id: string;
  object: string;
  created: number;
  model: string;
  choices: ChatChoice[];
  usage?: Usage;
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
  escrow_program_id?: string;
}

export interface PaymentRequired {
  x402_version: number;
  accepts: PaymentAccept[];
  cost_breakdown: CostBreakdown;
  error: string;
}

export interface ClientOptions {
  privateKey?: string;
  apiUrl?: string;
  sessionBudget?: number;
  timeout?: number;
}

export interface ModelInfo {
  id: string;
  object: string;
  owned_by: string;
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
