import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { ResponseCache } from '../../src/cache.js';
import { ChatMessage, ChatResponse, ChatChoice, Usage } from '../../src/types.js';

function makeResponse(content: string): ChatResponse {
  return new ChatResponse(
    'id-1',
    'chat.completion',
    Date.now(),
    'gpt-4',
    [new ChatChoice(0, new ChatMessage('assistant', content), 'stop')],
    new Usage(10, 5, 15),
  );
}

describe('ResponseCache', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('cacheKey is deterministic', () => {
    const messages = [new ChatMessage('user', 'Hello')];
    const k1 = ResponseCache.cacheKey('gpt-4', messages);
    const k2 = ResponseCache.cacheKey('gpt-4', messages);
    expect(k1).toBe(k2);
  });

  it('cacheKey differs for different models', () => {
    const messages = [new ChatMessage('user', 'Hello')];
    const k1 = ResponseCache.cacheKey('gpt-4', messages);
    const k2 = ResponseCache.cacheKey('gpt-3.5', messages);
    expect(k1).not.toBe(k2);
  });

  it('cacheKey differs for different messages', () => {
    const k1 = ResponseCache.cacheKey('gpt-4', [new ChatMessage('user', 'Hello')]);
    const k2 = ResponseCache.cacheKey('gpt-4', [new ChatMessage('user', 'World')]);
    expect(k1).not.toBe(k2);
  });

  it('put and get round-trip', () => {
    const cache = new ResponseCache();
    const key = ResponseCache.cacheKey('gpt-4', [new ChatMessage('user', 'Hi')]);
    const resp = makeResponse('Hello!');
    cache.put(key, resp);
    const got = cache.get(key);
    expect(got).toBeDefined();
    expect(got!.choices[0].message.content).toBe('Hello!');
  });

  it('get returns undefined for missing key', () => {
    const cache = new ResponseCache();
    expect(cache.get('nonexistent-key')).toBeUndefined();
  });

  it('entries expire after TTL', () => {
    const cache = new ResponseCache(200, 10);
    const key = ResponseCache.cacheKey('gpt-4', [new ChatMessage('user', 'Hi')]);
    cache.put(key, makeResponse('Hello'));
    expect(cache.get(key)).toBeDefined();

    vi.advanceTimersByTime(11_000);
    expect(cache.get(key)).toBeUndefined();
  });

  it('evicts oldest when maxEntries exceeded', () => {
    const cache = new ResponseCache(2, 600);
    const k1 = ResponseCache.cacheKey('m1', [new ChatMessage('user', 'a')]);
    const k2 = ResponseCache.cacheKey('m2', [new ChatMessage('user', 'b')]);
    const k3 = ResponseCache.cacheKey('m3', [new ChatMessage('user', 'c')]);

    cache.put(k1, makeResponse('r1'));
    cache.put(k2, makeResponse('r2'));
    cache.put(k3, makeResponse('r3'));

    expect(cache.get(k1)).toBeUndefined();
    expect(cache.get(k2)).toBeDefined();
    expect(cache.get(k3)).toBeDefined();
  });

  it('dedup window prevents re-caching', () => {
    const cache = new ResponseCache(200, 600, 30);
    const key = ResponseCache.cacheKey('gpt-4', [new ChatMessage('user', 'Hi')]);
    cache.put(key, makeResponse('r1'));

    // Within dedup window, put same key again
    cache.put(key, makeResponse('r2'));
    const got = cache.get(key);
    // Should still be the original
    expect(got!.choices[0].message.content).toBe('r1');
  });
});
