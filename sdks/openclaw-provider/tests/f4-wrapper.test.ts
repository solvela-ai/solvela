/**
 * Tests for the F4 settle-on-stream-completion wrapper.
 *
 * Verifies:
 *   - settle POST fires once when stream.result() resolves
 *   - settle POST carries model, agent, service_id, and actual token usage
 *   - settle POST status is 'completed' on normal stop, 'error' on aborted/error
 *   - stream consumer is never blocked or impacted by settle failure
 *   - timeoutMs is honored (AbortSignal fires)
 *   - stats counter tracks fired/succeeded/failed
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import {
  wrapStreamForF4,
  type F4Context,
  type F4Stats,
  type ResultableStream,
} from '../src/f4-wrapper.ts';

// Minimal AssistantMessage stub. Real type from @earendil-works/pi-ai has
// many more fields, but the F4 wrapper only reads stopReason + usage.
type FakeMessage = {
  stopReason: 'stop' | 'length' | 'toolUse' | 'error' | 'aborted';
  usage?: { input: number; output: number };
};

function makeStream(message: FakeMessage, opts: { delayMs?: number; rejectWith?: Error } = {}): ResultableStream {
  return {
    result: () =>
      new Promise((resolve, reject) => {
        const fire = () => {
          if (opts.rejectWith) reject(opts.rejectWith);
          else resolve(message as unknown as Parameters<typeof resolve>[0]);
        };
        if (opts.delayMs) setTimeout(fire, opts.delayMs);
        else fire();
      }),
  };
}

interface CapturedRequest {
  url: string;
  method: string;
  body: string;
}

function makeMockFetch(opts: {
  status?: number;
  throwOnCall?: Error;
  delayMs?: number;
} = {}): { fn: typeof fetch; captured: CapturedRequest[] } {
  const captured: CapturedRequest[] = [];
  const fn = (async (input: Parameters<typeof fetch>[0], init?: RequestInit) => {
    captured.push({
      url: typeof input === 'string' ? input : input.toString(),
      method: init?.method ?? 'GET',
      body: typeof init?.body === 'string' ? init.body : '',
    });
    if (opts.delayMs) await new Promise((r) => setTimeout(r, opts.delayMs));
    if (opts.throwOnCall) throw opts.throwOnCall;
    return new Response(null, { status: opts.status ?? 200 });
  }) as unknown as typeof fetch;
  return { fn, captured };
}

const baseF4: Omit<F4Context, 'fetchFn'> = {
  serviceId: 'svc_test_123',
  agentPubkey: 'GaTeWaYPuBkEyXXXXXXXXXXXXXXXXXXXXXXXXXXXXX',
  modelId: 'gpt-4o',
  gatewayUrl: 'https://gw.example.test',
};

/** Wait until the microtask queue + a setImmediate cycle drains. */
async function flushAsync(): Promise<void> {
  await new Promise<void>((resolve) => setImmediate(resolve));
  await new Promise<void>((resolve) => setImmediate(resolve));
}

describe('wrapStreamForF4', () => {
  it('returns the input stream unchanged (no piping)', () => {
    const stream = makeStream({ stopReason: 'stop' });
    const { fn } = makeMockFetch();
    const result = wrapStreamForF4(stream, { ...baseF4, fetchFn: fn });
    assert.strictEqual(result, stream, 'wrapStreamForF4 must return the same reference');
  });

  it('fires settle POST with completed status on normal stream completion', async () => {
    const stream = makeStream({
      stopReason: 'stop',
      usage: { input: 17, output: 42 },
    });
    const { fn, captured } = makeMockFetch();
    const stats: F4Stats = { fired: 0, succeeded: 0, failed: 0 };

    wrapStreamForF4(stream, { ...baseF4, fetchFn: fn }, stats);
    await flushAsync();

    assert.strictEqual(captured.length, 1, 'exactly one settle POST');
    const req = captured[0];
    assert.strictEqual(req.url, 'https://gw.example.test/v1/escrow/settle');
    assert.strictEqual(req.method, 'POST');
    const body = JSON.parse(req.body);
    assert.strictEqual(body.service_id, 'svc_test_123');
    assert.strictEqual(body.agent_pubkey, baseF4.agentPubkey);
    assert.strictEqual(body.model, 'gpt-4o');
    assert.strictEqual(body.status, 'completed');
    assert.strictEqual(body.actual_prompt_tokens, 17);
    assert.strictEqual(body.actual_completion_tokens, 42);
    assert.strictEqual(stats.fired, 1);
    assert.strictEqual(stats.succeeded, 1);
    assert.strictEqual(stats.failed, 0);
  });

  it("settle POST status='error' on stopReason='error'", async () => {
    const stream = makeStream({ stopReason: 'error', usage: { input: 5, output: 0 } });
    const { fn, captured } = makeMockFetch();

    wrapStreamForF4(stream, { ...baseF4, fetchFn: fn });
    await flushAsync();

    const body = JSON.parse(captured[0].body);
    assert.strictEqual(body.status, 'error');
  });

  it("settle POST status='error' on stopReason='aborted'", async () => {
    const stream = makeStream({ stopReason: 'aborted' });
    const { fn, captured } = makeMockFetch();

    wrapStreamForF4(stream, { ...baseF4, fetchFn: fn });
    await flushAsync();

    const body = JSON.parse(captured[0].body);
    assert.strictEqual(body.status, 'error');
  });

  it('skips token fields when usage is absent', async () => {
    const stream = makeStream({ stopReason: 'stop' });
    const { fn, captured } = makeMockFetch();

    wrapStreamForF4(stream, { ...baseF4, fetchFn: fn });
    await flushAsync();

    const body = JSON.parse(captured[0].body);
    assert.ok(!('actual_prompt_tokens' in body), 'omit prompt tokens when usage absent');
    assert.ok(!('actual_completion_tokens' in body), 'omit completion tokens when usage absent');
  });

  it('does not throw or block when stream.result() rejects', async () => {
    const stream = makeStream({ stopReason: 'stop' }, { rejectWith: new Error('stream burned down') });
    const { fn, captured } = makeMockFetch();
    const stats: F4Stats = { fired: 0, succeeded: 0, failed: 0 };

    assert.doesNotThrow(() => wrapStreamForF4(stream, { ...baseF4, fetchFn: fn }, stats));
    await flushAsync();

    assert.strictEqual(captured.length, 0, 'no settle POST when stream itself failed before result');
    assert.strictEqual(stats.fired, 0, 'fired counter must NOT increment when stream rejected');
  });

  it('does not throw or block when settle POST returns 404 (gateway endpoint not deployed)', async () => {
    const stream = makeStream({ stopReason: 'stop' });
    const { fn } = makeMockFetch({ status: 404 });
    const stats: F4Stats = { fired: 0, succeeded: 0, failed: 0 };

    assert.doesNotThrow(() => wrapStreamForF4(stream, { ...baseF4, fetchFn: fn }, stats));
    await flushAsync();

    assert.strictEqual(stats.fired, 1, 'fired counter increments even on 404');
    assert.strictEqual(stats.succeeded, 0, 'succeeded does NOT increment on 404');
    assert.strictEqual(stats.failed, 1, 'failed increments on 404');
  });

  it('does not throw or block when settle POST throws (network error)', async () => {
    const stream = makeStream({ stopReason: 'stop' });
    const { fn } = makeMockFetch({ throwOnCall: new Error('ECONNREFUSED') });
    const stats: F4Stats = { fired: 0, succeeded: 0, failed: 0 };

    assert.doesNotThrow(() => wrapStreamForF4(stream, { ...baseF4, fetchFn: fn }, stats));
    await flushAsync();

    assert.strictEqual(stats.fired, 1);
    assert.strictEqual(stats.failed, 1);
  });

  it('accepts a Promise<ResultableStream> as input', async () => {
    const stream = makeStream({ stopReason: 'stop', usage: { input: 1, output: 1 } });
    const streamPromise = Promise.resolve(stream);
    const { fn, captured } = makeMockFetch();

    wrapStreamForF4(streamPromise, { ...baseF4, fetchFn: fn });
    await flushAsync();

    assert.strictEqual(captured.length, 1, 'settle fires after the stream promise resolves');
  });

  it('respects custom timeoutMs (AbortSignal fires)', async () => {
    const stream = makeStream({ stopReason: 'stop' });
    // Mock fetch that ignores abort and resolves slowly — should be aborted
    let aborted = false;
    const fn = (async (_input: Parameters<typeof fetch>[0], init?: RequestInit) => {
      return new Promise<Response>((resolve, reject) => {
        const signal = init?.signal;
        const timer = setTimeout(() => resolve(new Response(null, { status: 200 })), 1000);
        if (signal) {
          signal.addEventListener('abort', () => {
            aborted = true;
            clearTimeout(timer);
            reject(new Error('aborted'));
          });
        }
      });
    }) as unknown as typeof fetch;

    const stats: F4Stats = { fired: 0, succeeded: 0, failed: 0 };
    wrapStreamForF4(stream, { ...baseF4, fetchFn: fn, timeoutMs: 20 }, stats);

    // Wait long enough for timeout to fire but not the 1s resolve
    await new Promise((r) => setTimeout(r, 100));

    assert.strictEqual(aborted, true, 'AbortSignal must fire after timeoutMs');
    assert.strictEqual(stats.failed, 1, 'aborted settle counts as failed');
  });
});
