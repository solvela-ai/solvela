import { LLMClient } from './client';
import type { ChatMessage, ChatResponse, ClientOptions } from './types';

/**
 * OpenAI-compatible drop-in replacement backed by RustyClawRouter.
 *
 * This lets existing code that uses the OpenAI SDK pattern switch to
 * paying with USDC on Solana with a one-line import change:
 *
 *   // Before:
 *   import OpenAI from 'openai';
 *
 *   // After:
 *   import { OpenAI } from '@rustyclawrouter/sdk';
 *
 *   const client = new OpenAI({ apiUrl: 'http://localhost:8402' });
 *   const resp = await client.chat.completions.create({
 *     model: 'gpt-4o',
 *     messages: [{ role: 'user', content: 'Hello!' }],
 *   });
 *   console.log(resp.choices[0].message.content);
 */
export class OpenAI {
  chat: { completions: Completions };

  private client: LLMClient;

  constructor(options: ClientOptions = {}) {
    this.client = new LLMClient(options);
    this.chat = { completions: new Completions(this.client) };
  }

  /** Access the underlying LLMClient for advanced features (budget, health, etc.). */
  getClient(): LLMClient {
    return this.client;
  }
}

class Completions {
  constructor(private client: LLMClient) {}

  /**
   * Create a chat completion — mirrors the OpenAI SDK signature.
   */
  async create(params: {
    model: string;
    messages: ChatMessage[];
    max_tokens?: number;
    temperature?: number;
    stream?: boolean;
  }): Promise<ChatResponse> {
    return this.client.chatCompletion({
      model: params.model,
      messages: params.messages,
      maxTokens: params.max_tokens,
      temperature: params.temperature,
      stream: params.stream,
    });
  }
}
