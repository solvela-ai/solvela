import { describe, it, expect } from 'vitest';
import { SolvelaClient } from '../../src/client.js';
import { ChatRequest, ChatMessage } from '../../src/types.js';

const LIVE = process.env.SOLVELA_LIVE_TESTS === '1';

describe.skipIf(!LIVE)('Live gateway tests', () => {
  const gatewayUrl = process.env.SOLVELA_GATEWAY_URL || 'http://localhost:8402';

  it('health endpoint responds', async () => {
    const resp = await fetch(`${gatewayUrl}/health`);
    expect(resp.status).toBe(200);
  });

  it('models endpoint returns data', async () => {
    const client = new SolvelaClient({ config: { gatewayUrl } });
    const models = await client.models();
    expect(models.length).toBeGreaterThan(0);
  });

  it('chat without payment returns 402 or succeeds with free model', async () => {
    const client = new SolvelaClient({
      config: { gatewayUrl, freeFallbackModel: 'deepseek-chat' },
    });
    const request = new ChatRequest('gpt-4', [
      new ChatMessage('user', 'Say hello in one word.'),
    ]);

    // This should either succeed with free fallback or throw PaymentRequired
    try {
      const resp = await client.chat(request);
      expect(resp.choices.length).toBeGreaterThan(0);
    } catch (e) {
      expect((e as Error).name).toBe('PaymentRequiredError');
    }
  });
});
