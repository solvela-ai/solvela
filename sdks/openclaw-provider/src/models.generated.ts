// DO NOT EDIT — regenerate via: npm run generate:models
// Source: config/models.toml
//
// This file is committed so the build does not require @iarna/toml at runtime.

export interface SolvelaModel {
  /** Namespaced model ID: "solvela/<gateway-model-id>" */
  id: string;
  /** Display name shown in OpenClaw model picker */
  name: string;
  /** Upstream provider (openai | anthropic | google | deepseek | xai) */
  provider: string;
  contextWindow: number;
  maxTokens: number;
  /** Provider cost per million input tokens (before 5% Solvela fee) */
  inputCostPerMillion: number;
  /** Provider cost per million output tokens (before 5% Solvela fee) */
  outputCostPerMillion: number;
  supportsStreaming: boolean;
  supportsTools?: boolean;
  supportsVision?: boolean;
  supportsStructuredOutput?: boolean;
  reasoning?: boolean;
}

/**
 * All Solvela models generated from config/models.toml.
 * Routing profiles (solvela/auto, solvela/eco, etc.) are added separately
 * by registry.ts and are NOT included here.
 */
export const SOLVELA_MODELS: SolvelaModel[] = [
  {
    "id": "solvela/gpt-5.2",
    "name": "GPT-5.2",
    "provider": "openai",
    "contextWindow": 400000,
    "maxTokens": 32768,
    "inputCostPerMillion": 1.75,
    "outputCostPerMillion": 14,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/gpt-4o",
    "name": "GPT-4o",
    "provider": "openai",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 2.5,
    "outputCostPerMillion": 10,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/gpt-4o-mini",
    "name": "GPT-4o Mini",
    "provider": "openai",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.15,
    "outputCostPerMillion": 0.6,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/o3",
    "name": "o3",
    "provider": "openai",
    "contextWindow": 200000,
    "maxTokens": 32768,
    "inputCostPerMillion": 2,
    "outputCostPerMillion": 8,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/gpt-oss-120b",
    "name": "GPT-OSS 120B",
    "provider": "openai",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0,
    "outputCostPerMillion": 0,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/claude-opus-4-20250514",
    "name": "Claude Opus 4.6",
    "provider": "anthropic",
    "contextWindow": 200000,
    "maxTokens": 32768,
    "inputCostPerMillion": 5,
    "outputCostPerMillion": 25,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/claude-sonnet-4-20250514",
    "name": "Claude Sonnet 4.6",
    "provider": "anthropic",
    "contextWindow": 200000,
    "maxTokens": 32768,
    "inputCostPerMillion": 3,
    "outputCostPerMillion": 15,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/claude-haiku-4-5-20251001",
    "name": "Claude Haiku 4.5",
    "provider": "anthropic",
    "contextWindow": 200000,
    "maxTokens": 32768,
    "inputCostPerMillion": 1,
    "outputCostPerMillion": 5,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/gemini-3.1-pro",
    "name": "Gemini 3.1 Pro",
    "provider": "google",
    "contextWindow": 1000000,
    "maxTokens": 32768,
    "inputCostPerMillion": 2,
    "outputCostPerMillion": 12,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/gemini-2.5-flash",
    "name": "Gemini 2.5 Flash",
    "provider": "google",
    "contextWindow": 1000000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.3,
    "outputCostPerMillion": 2.5,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/gemini-2.5-flash-lite",
    "name": "Gemini 2.5 Flash Lite",
    "provider": "google",
    "contextWindow": 1000000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.1,
    "outputCostPerMillion": 0.4,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/deepseek-chat",
    "name": "DeepSeek V3.2 Chat",
    "provider": "deepseek",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.28,
    "outputCostPerMillion": 0.42,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/deepseek-reasoner",
    "name": "DeepSeek V3.2 Reasoner",
    "provider": "deepseek",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.28,
    "outputCostPerMillion": 0.42,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/grok-4-fast-reasoning",
    "name": "Grok 4 Fast (Reasoning)",
    "provider": "xai",
    "contextWindow": 2000000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.2,
    "outputCostPerMillion": 0.5,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  },
  {
    "id": "solvela/grok-code-fast-1",
    "name": "Grok Code Fast",
    "provider": "xai",
    "contextWindow": 256000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.2,
    "outputCostPerMillion": 1.5,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/o3-mini",
    "name": "o3 Mini",
    "provider": "openai",
    "contextWindow": 200000,
    "maxTokens": 100000,
    "inputCostPerMillion": 1.1,
    "outputCostPerMillion": 4.4,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": true,
    "reasoning": true
  },
  {
    "id": "solvela/o4-mini",
    "name": "o4 Mini",
    "provider": "openai",
    "contextWindow": 200000,
    "maxTokens": 100000,
    "inputCostPerMillion": 1.1,
    "outputCostPerMillion": 4.4,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": true,
    "reasoning": true
  },
  {
    "id": "solvela/gpt-4.1",
    "name": "GPT-4.1",
    "provider": "openai",
    "contextWindow": 1047576,
    "maxTokens": 32768,
    "inputCostPerMillion": 2,
    "outputCostPerMillion": 8,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": true,
    "reasoning": false
  },
  {
    "id": "solvela/gpt-4.1-mini",
    "name": "GPT-4.1 Mini",
    "provider": "openai",
    "contextWindow": 1047576,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.4,
    "outputCostPerMillion": 1.6,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": true,
    "reasoning": false
  },
  {
    "id": "solvela/gpt-4.1-nano",
    "name": "GPT-4.1 Nano",
    "provider": "openai",
    "contextWindow": 1047576,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.1,
    "outputCostPerMillion": 0.4,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": true,
    "reasoning": false
  },
  {
    "id": "solvela/gemini-2.0-flash",
    "name": "Gemini 2.0 Flash",
    "provider": "google",
    "contextWindow": 1000000,
    "maxTokens": 8192,
    "inputCostPerMillion": 0.1,
    "outputCostPerMillion": 0.4,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/gemini-2.0-flash-lite",
    "name": "Gemini 2.0 Flash Lite",
    "provider": "google",
    "contextWindow": 1000000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.075,
    "outputCostPerMillion": 0.3,
    "supportsStreaming": true,
    "supportsTools": false,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/deepseek-coder",
    "name": "DeepSeek Coder V3",
    "provider": "deepseek",
    "contextWindow": 128000,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.28,
    "outputCostPerMillion": 0.42,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/grok-3",
    "name": "Grok 3",
    "provider": "xai",
    "contextWindow": 131072,
    "maxTokens": 32768,
    "inputCostPerMillion": 3,
    "outputCostPerMillion": 15,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": true,
    "supportsStructuredOutput": false,
    "reasoning": false
  },
  {
    "id": "solvela/grok-3-mini",
    "name": "Grok 3 Mini",
    "provider": "xai",
    "contextWindow": 131072,
    "maxTokens": 32768,
    "inputCostPerMillion": 0.3,
    "outputCostPerMillion": 0.5,
    "supportsStreaming": true,
    "supportsTools": true,
    "supportsVision": false,
    "supportsStructuredOutput": false,
    "reasoning": true
  }
];

export const MODEL_COUNT = 25;
