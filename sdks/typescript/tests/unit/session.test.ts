import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { SessionStore } from '../../src/session.js';
import { ChatMessage } from '../../src/types.js';

describe('SessionStore', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('getOrCreate creates a new session', () => {
    const store = new SessionStore();
    const info = store.getOrCreate('s1', 'gpt-4');
    expect(info.sessionId).toBe('s1');
    expect(info.model).toBe('gpt-4');
    expect(info.requestCount).toBe(0);
  });

  it('getOrCreate returns existing session', () => {
    const store = new SessionStore();
    const info1 = store.getOrCreate('s1', 'gpt-4');
    store.recordRequest('s1', 'hash-123');
    const info2 = store.getOrCreate('s1', 'gpt-3.5');
    // Should keep original model, and request count should reflect the record
    expect(info2.model).toBe('gpt-4');
    expect(info2.requestCount).toBe(1);
  });

  it('recordRequest increments count and tracks hash', () => {
    const store = new SessionStore();
    store.getOrCreate('s1', 'gpt-4');
    store.recordRequest('s1', 'hash-111');
    store.recordRequest('s1', 'hash-222');
    const info = store.getOrCreate('s1', 'gpt-4');
    expect(info.requestCount).toBe(2);
  });

  it('cleanupExpired removes old sessions', () => {
    const store = new SessionStore(10);
    store.getOrCreate('s1', 'gpt-4');
    expect(store.getOrCreate('s1', 'gpt-4').requestCount).toBe(0);

    vi.advanceTimersByTime(11_000);
    store.cleanupExpired();

    // After cleanup, it should create a brand new session
    const info = store.getOrCreate('s1', 'gpt-4');
    expect(info.requestCount).toBe(0);
  });

  it('deriveSessionId is deterministic', () => {
    const msgs = [
      new ChatMessage('system', 'You are helpful'),
      new ChatMessage('user', 'Hello'),
    ];
    const id1 = SessionStore.deriveSessionId(msgs);
    const id2 = SessionStore.deriveSessionId(msgs);
    expect(id1).toBe(id2);
    expect(id1.length).toBeGreaterThan(0);
  });

  it('deriveSessionId differs for different messages', () => {
    const id1 = SessionStore.deriveSessionId([new ChatMessage('user', 'Hello')]);
    const id2 = SessionStore.deriveSessionId([new ChatMessage('user', 'World')]);
    expect(id1).not.toBe(id2);
  });
});
