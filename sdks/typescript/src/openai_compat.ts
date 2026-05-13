import { SolvelaClient } from './client.js';
import { ChatChunk, ChatMessage, ChatRequest, ChatResponse, Role } from './types.js';

interface CreateParams {
  model: string;
  messages: Array<{ role: string; content: string | null; name?: string }>;
  stream?: boolean;
  max_tokens?: number;
  temperature?: number;
  top_p?: number;
}

export class OpenAICompat {
  chat: {
    completions: {
      create: (
        params: CreateParams,
      ) => Promise<ChatResponse | AsyncGenerator<ChatChunk>>;
    };
  };

  constructor(client: SolvelaClient) {
    this.chat = {
      completions: {
        create: async (
          params: CreateParams,
        ): Promise<ChatResponse | AsyncGenerator<ChatChunk>> => {
          const messages = params.messages.map(
            (m) => new ChatMessage(m.role as Role, m.content, m.name),
          );
          const req = new ChatRequest(
            params.model,
            messages,
            params.max_tokens,
            params.temperature,
            params.top_p,
            params.stream ?? false,
          );
          if (params.stream) {
            return client.chatStream(req);
          }
          return client.chat(req);
        },
      },
    };
  }
}
