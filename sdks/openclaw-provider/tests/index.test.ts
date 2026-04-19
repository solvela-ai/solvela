/**
 * Integration tests for the plugin registration and wrapStreamFn hook.
 *
 * Mocks the OpenClaw `api` object and a fake gateway to test:
 *   1. Plugin registration calls api.registerProvider with correct config
 *   2. wrapStreamFn injects payment-signature header (non-stub base64)
 *   3. wrapStreamFn throws when gateway returns 200 without SOLVELA_ALLOW_DEV_BYPASS=1 (HF-P3-C4)
 *   4. wrapStreamFn passes through when SOLVELA_ALLOW_DEV_BYPASS=1 is set
 *   5. wrapStreamFn throws when ctx.streamFn is absent (HF-P3-C3)
 *   6. Catalog returns shadow model when SOLANA_WALLET_KEY is missing (HF-P3-H5)
 *   7. wrapStreamFn body frozen back to params (HF-P3-C1)
 *   8. wrapStreamFn refunds budget on inner() failure (HF-P3-C2)
 *   9. getSigningMode() defaults to 'direct' (HF-P3-H6)
 *  10. resolveDynamicModel throws on unknown solvela/ prefix (HF-P3-H4)
 */

import { describe, it, before, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { resolve, dirname } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createServer } from 'node:http';
import type { Server } from 'node:http';

import register, { GatewayAcceptedWithoutPayment } from '../src/index.ts';
import { SolvelaSigner } from '../src/signer.ts';
import type { OpenClawApi, ProviderConfig, StreamFnContext } from '../src/openclaw-types.ts';

// A non-stub header returned by the DI signer in integration tests.
// The real SDK returns stubs for fake keys, so integration tests use DI to
// inject a signer that returns a known good-looking header.
const REAL_LOOKING_HEADER = Buffer.from(
  JSON.stringify({
    x402_version: 2,
    resource: { url: 'https://api.solvela.ai/v1/chat/completions', method: 'POST' },
    accepted: { scheme: 'exact', network: 'solana' },
    payload: { transaction: 'REAL_TX_NOT_STUB_xABCDEF1234567890' },
  }),
  'utf-8',
).toString('base64');

/** Create a SolvelaSigner with DI that returns a non-stub header. */
function makeDISigner(opts: ConstructorParameters<typeof SolvelaSigner>[0] = {}): SolvelaSigner {
  return new SolvelaSigner({
    ...opts,
    _createPaymentHeaderFn: async () => REAL_LOOKING_HEADER,
  } as ConstructorParameters<typeof SolvelaSigner>[0] & { _createPaymentHeaderFn: () => Promise<string> });
}

const __dirname = dirname(fileURLToPath(import.meta.url));

const mock402 = JSON.parse(
  readFileSync(resolve(__dirname, 'fixtures/mock-402.json'), 'utf-8'),
);

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/** Capture the provider config passed to api.registerProvider */
function makeMockApi(): { api: OpenClawApi; config: ProviderConfig | null } {
  const state: { config: ProviderConfig | null } = { config: null };
  const api: OpenClawApi = {
    registerProvider(cfg: ProviderConfig) {
      state.config = cfg;
    },
  };
  return { api, config: state as unknown as ProviderConfig | null };
}

/**
 * Spin up a mock HTTP server on a random OS-assigned port.
 * Returns { server, port } after the server is listening.
 */
function startMockServer(
  handler: (res: import('node:http').ServerResponse) => void,
): Promise<{ server: Server; port: number }> {
  return new Promise((resolve, reject) => {
    const server = createServer((_req, res) => handler(res));
    server.listen(0, '127.0.0.1', () => {
      const addr = server.address();
      if (!addr || typeof addr === 'string') {
        reject(new Error('could not get server port'));
        return;
      }
      resolve({ server, port: addr.port });
    });
    server.on('error', reject);
  });
}

/** Spin up a mock HTTP server that serves a 402 on all requests */
async function startMock402Server(): Promise<{ server: Server; port: number }> {
  return startMockServer((res) => {
    res.writeHead(402, { 'content-type': 'application/json' });
    res.end(JSON.stringify(mock402));
  });
}

/** Spin up a mock HTTP server that serves 200 (dev_bypass mode) */
async function startMock200Server(): Promise<{ server: Server; port: number }> {
  return startMockServer((res) => {
    res.writeHead(200, { 'content-type': 'application/json' });
    res.end(JSON.stringify({ id: 'test', object: 'chat.completion', created: 0, model: 'auto', choices: [] }));
  });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

before(() => {
  delete process.env['SOLANA_WALLET_KEY'];
  delete process.env['SOLANA_RPC_URL'];
  delete process.env['SOLVELA_API_URL'];
  delete process.env['SOLVELA_SIGNING_MODE'];
  delete process.env['SOLVELA_SESSION_BUDGET'];
  delete process.env['SOLVELA_ALLOW_DEV_BYPASS'];
});

afterEach(() => {
  delete process.env['SOLANA_WALLET_KEY'];
  delete process.env['SOLANA_RPC_URL'];
  delete process.env['SOLVELA_API_URL'];
  delete process.env['SOLVELA_SIGNING_MODE'];
  delete process.env['SOLVELA_SESSION_BUDGET'];
  delete process.env['SOLVELA_ALLOW_DEV_BYPASS'];
});

describe('Plugin registration', () => {
  it('calls api.registerProvider with id "solvela"', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    assert.ok(cfg !== null, 'registerProvider must be called');
    assert.strictEqual(cfg.id, 'solvela');
  });

  it('declares catalog, wrapStreamFn, and resolveDynamicModel hooks', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    assert.ok(typeof cfg.catalog?.run === 'function', 'catalog.run must be a function');
    assert.ok(typeof cfg.wrapStreamFn === 'function', 'wrapStreamFn must be a function');
    assert.ok(typeof cfg.resolveDynamicModel === 'function', 'resolveDynamicModel must be a function');
  });

  it('throws if api.registerProvider is not a function (HF-P3-C3)', () => {
    assert.throws(
      () => register({} as OpenClawApi),
      (err: Error) => {
        assert.ok(err.message.includes('api.registerProvider is not a function'));
        return true;
      },
    );
  });

  it('catalog returns shadow model when SOLANA_WALLET_KEY is missing (HF-P3-H5)', async () => {
    delete process.env['SOLANA_WALLET_KEY'];
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const mockCtx = { resolveProviderApiKey: () => ({ apiKey: undefined }) };
    const result = await cfg.catalog.run(mockCtx);
    assert.ok(result !== null, 'catalog.run must return shadow model, not null');
    assert.strictEqual(result.provider.models.length, 1, 'shadow catalog must have exactly 1 model');
    assert.strictEqual(result.provider.models[0].id, 'solvela/not-configured');
    assert.ok(
      result.provider.models[0].name.includes('SOLANA_WALLET_KEY'),
      'shadow model name must mention SOLANA_WALLET_KEY',
    );
  });

  it('catalog returns provider config when SOLANA_WALLET_KEY is set', async () => {
    process.env['SOLANA_WALLET_KEY'] = 'fake_key_for_test';
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const mockCtx = { resolveProviderApiKey: () => ({ apiKey: 'fake_key_for_test' }) };
    const result = await cfg.catalog.run(mockCtx);
    assert.ok(result !== null, 'catalog.run must return config when key is set');
    assert.strictEqual(result!.provider.api, 'openai-completions');
    assert.ok(Array.isArray(result!.provider.models), 'models must be an array');
    assert.ok(result!.provider.models.length > 1, 'models must be non-empty (more than shadow)');
  });

  it('catalog apiKey is empty string (HF-P3-L5)', async () => {
    process.env['SOLANA_WALLET_KEY'] = 'fake_key';
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const result = await cfg.catalog.run({ resolveProviderApiKey: () => ({ apiKey: 'k' }) });
    assert.strictEqual(result!.provider.apiKey, '', 'apiKey must be empty string, not placeholder');
  });

  it('catalog models include routing profiles and real models', async () => {
    process.env['SOLANA_WALLET_KEY'] = 'fake_key_for_test';
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const result = await cfg.catalog.run({ resolveProviderApiKey: () => ({ apiKey: 'k' }) });
    const ids = result!.provider.models.map((m) => m.id);
    assert.ok(ids.includes('solvela/auto'), 'must include routing profile solvela/auto');
    assert.ok(ids.some((id) => id.startsWith('solvela/') && !['solvela/auto','solvela/eco','solvela/premium','solvela/free'].includes(id)), 'must include real model IDs');
  });

  it('default signing mode is direct (HF-P3-H6)', () => {
    // signingMode env not set — default must be direct
    delete process.env['SOLVELA_SIGNING_MODE'];
    // register() logs signingMode — capture stderr to verify
    const lines: string[] = [];
    const origWrite = process.stderr.write.bind(process.stderr);
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    (process.stderr as any).write = (chunk: string | Uint8Array, cb?: (err?: Error | null) => void): boolean => {
      if (typeof chunk === 'string') lines.push(chunk);
      return origWrite(chunk, cb as (err?: Error | null) => void);
    };
    try {
      const { api } = makeMockApi();
      register(api);
    } finally {
      // eslint-disable-next-line @typescript-eslint/no-explicit-any
      (process.stderr as any).write = origWrite;
    }
    const registerLine = lines.find((l) => l.includes('signingMode='));
    assert.ok(registerLine, 'must log signingMode at register time');
    assert.ok(registerLine!.includes('signingMode=direct'), `expected signingMode=direct, got: ${registerLine}`);
  });

  it('getSigningMode throws on unrecognized value (HF-P3-M8)', () => {
    process.env['SOLVELA_SIGNING_MODE'] = 'turbo';
    const { api } = makeMockApi();
    assert.throws(
      () => register(api),
      (err: Error) => {
        assert.ok(err.message.includes("'turbo' is not recognized"), `got: ${err.message}`);
        assert.ok(err.message.includes('auto|escrow|direct|off'), `expected off in accepted list, got: ${err.message}`);
        return true;
      },
    );
  });

  it("signingMode='off' accepted without throwing (parity with Phase 1 MCP)", () => {
    process.env['SOLVELA_SIGNING_MODE'] = 'off';
    const { api } = makeMockApi();
    // Should not throw — 'off' is a valid mode
    assert.doesNotThrow(() => register(api));
  });

  it("signingMode='off' skips signing — inner() called without payment-signature header", async () => {
    process.env['SOLVELA_SIGNING_MODE'] = 'off';
    // No SOLVELA_API_URL needed — probe is never made in 'off' mode
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    let capturedParams: { headers: Record<string, string> } | null = null;
    const mockStreamFn = async (params: { headers: Record<string, string> }) => {
      capturedParams = params;
      return { status: 200 };
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    await wrapped({
      headers: { 'content-type': 'application/json' },
      body: '{"model":"auto","messages":[]}',
      url: 'http://127.0.0.1:9999/v1/chat/completions',
    });

    assert.ok(capturedParams !== null, 'inner stream function must be called');
    const captured = capturedParams as { headers: Record<string, string> };
    assert.ok(
      !('payment-signature' in captured.headers),
      'payment-signature must NOT be injected in off mode',
    );
  });
});

describe('wrapStreamFn', () => {
  it('throws when ctx.streamFn is absent (HF-P3-C3)', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const ctx: StreamFnContext = {};
    assert.throws(
      () => cfg.wrapStreamFn!(ctx),
      (err: Error) => {
        assert.ok(err.message.includes('wrapStreamFn invoked without a valid streamFn'));
        return true;
      },
    );
  });

  it('injects payment-signature header when gateway returns 402', async () => {
    const { server, port } = await startMock402Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    process.env['SOLANA_WALLET_KEY'] = 'fake_wallet_key_for_test';

    const { api, config } = makeMockApi();
    // DI signer bypasses real SDK signing — returns known non-stub header
    register(api, { _signer: makeDISigner() });
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    let capturedParams: { headers: Record<string, string> } | null = null;
    const mockStreamFn = async (params: { headers: Record<string, string> }) => {
      capturedParams = params;
      return { status: 200 };
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;
    assert.ok(typeof wrapped === 'function', 'wrapStreamFn must return a function');

    try {
      await wrapped({
        headers: { 'content-type': 'application/json' },
        body: '{"model":"auto","messages":[]}',
        url: `http://127.0.0.1:${port}/v1/chat/completions`,
      });
    } finally {
      server.close();
    }

    assert.ok(capturedParams !== null, 'inner stream function must be called');
    const captured = capturedParams as { headers: Record<string, string> };
    assert.ok(
      'payment-signature' in captured.headers,
      'payment-signature header must be injected',
    );
    const header = captured.headers['payment-signature'];
    assert.ok(typeof header === 'string' && header.length > 10, 'payment-signature must be a non-trivial base64 string');

    // Verify it decodes to valid JSON with x402_version
    const decoded = JSON.parse(Buffer.from(header, 'base64').toString('utf-8'));
    assert.ok(decoded.x402_version !== undefined, 'decoded header must have x402_version');
  });

  it('body written back to params as canonical string (HF-P3-C1)', async () => {
    const { server, port } = await startMock402Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    process.env['SOLANA_WALLET_KEY'] = 'fake_wallet_key_for_test';

    const { api, config } = makeMockApi();
    register(api, { _signer: makeDISigner() });
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    let capturedParams: { body?: unknown } | null = null;
    const mockStreamFn = async (params: { body?: unknown }) => {
      capturedParams = params;
      return { status: 200 };
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    const originalBody = '{"model":"auto","messages":[]}';
    const inParams = {
      headers: { 'content-type': 'application/json' },
      body: originalBody,
      url: `http://127.0.0.1:${port}/v1/chat/completions`,
    };
    try {
      await wrapped(inParams);
    } finally {
      server.close();
    }

    // params.body must be the canonical string (HF-P3-C1)
    assert.strictEqual(inParams.body, originalBody, 'params.body must be set to canonical string');
    assert.strictEqual(capturedParams!.body, originalBody, 'inner() params.body must match canonical string');
  });

  it('object body is stringified to canonical JSON string (HF-P3-C1 object branch)', async () => {
    const { server, port } = await startMock402Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    process.env['SOLANA_WALLET_KEY'] = 'fake_wallet_key_for_test';

    const { api, config } = makeMockApi();
    register(api, { _signer: makeDISigner() });
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    let capturedParams: { body?: unknown } | null = null;
    const mockStreamFn = async (params: { body?: unknown }) => {
      capturedParams = params;
      return { status: 200 };
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    const objectBody = { model: 'auto', messages: [{ role: 'user', content: 'hi' }] };
    const inParams: { headers: Record<string, string>; body: unknown; url: string } = {
      headers: { 'content-type': 'application/json' },
      body: objectBody,
      url: `http://127.0.0.1:${port}/v1/chat/completions`,
    };
    try {
      await wrapped(inParams as Parameters<typeof wrapped>[0]);
    } finally {
      server.close();
    }

    // After the call, params.body must be a string (not the original object)
    assert.strictEqual(typeof inParams.body, 'string', 'object body must be stringified to string');
    const expectedJson = JSON.stringify(objectBody);
    assert.strictEqual(inParams.body, expectedJson, 'body must be JSON.stringify of original object');
    assert.strictEqual(capturedParams!.body, expectedJson, 'inner() must receive the canonical string');
  });

  it('refunds budget on inner() failure (HF-P3-C2)', async () => {
    const { server: server1, port } = await startMock402Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    process.env['SOLANA_WALLET_KEY'] = 'fake_wallet_key_for_test';
    process.env['SOLVELA_SESSION_BUDGET'] = '10.00';

    const diSigner = makeDISigner({ sessionBudget: 10.00 });
    const { api, config } = makeMockApi();
    register(api, { _signer: diSigner });
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    // inner() always throws
    const mockStreamFn = async () => {
      throw new Error('network error');
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    // Should throw from inner() but NOT from budget exceeded
    await assert.rejects(
      () =>
        wrapped({
          headers: {},
          body: '{"model":"auto","messages":[]}',
          url: `http://127.0.0.1:${port}/v1/chat/completions`,
        }),
      (err: Error) => {
        assert.ok(err.message === 'network error', `expected network error, got: ${err.message}`);
        return true;
      },
    );

    server1.close();

    // The budget was refunded — second call should NOT hit budget exceeded.
    // Start a fresh 402 server on a new random port.
    const { server: server2, port: port2 } = await startMock402Server();
    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port2}`;

    let secondCallSucceeded = false;
    const mockStreamFn2 = async () => {
      secondCallSucceeded = true;
      return { status: 200 };
    };
    const ctx2: StreamFnContext = { streamFn: mockStreamFn2 as unknown as typeof mockStreamFn2 };
    const wrapped2 = cfg.wrapStreamFn!(ctx2)!;
    try {
      await wrapped2({
        headers: {},
        body: '{"model":"auto","messages":[]}',
        url: `http://127.0.0.1:${port2}/v1/chat/completions`,
      });
    } finally {
      server2.close();
    }
    assert.ok(secondCallSucceeded, 'second call must succeed — budget must have been refunded');
  });

  it('throws when gateway returns 200 without SOLVELA_ALLOW_DEV_BYPASS=1 (HF-P3-C4)', async () => {
    const { server, port } = await startMock200Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    delete process.env['SOLVELA_ALLOW_DEV_BYPASS'];

    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    const mockStreamFn = async () => ({ status: 200 });
    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    try {
      await assert.rejects(
        () =>
          wrapped({
            headers: { 'content-type': 'application/json' },
            body: '{"model":"auto","messages":[]}',
            url: `http://127.0.0.1:${port}/v1/chat/completions`,
          }),
        (err: Error) => {
          assert.ok(
            err.message.includes('SOLVELA_ALLOW_DEV_BYPASS=1'),
            `expected dev bypass error, got: ${err.message}`,
          );
          return true;
        },
      );
    } finally {
      server.close();
    }
  });

  it('passes through without signing when SOLVELA_ALLOW_DEV_BYPASS=1 (HF-P3-C4)', async () => {
    const { server, port } = await startMock200Server();

    process.env['SOLVELA_API_URL'] = `http://127.0.0.1:${port}`;
    process.env['SOLVELA_ALLOW_DEV_BYPASS'] = '1';

    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;

    let capturedParams: { headers: Record<string, string> } | null = null;
    const mockStreamFn = async (params: { headers: Record<string, string> }) => {
      capturedParams = params;
      return { status: 200 };
    };

    const ctx: StreamFnContext = { streamFn: mockStreamFn as unknown as typeof mockStreamFn };
    const wrapped = cfg.wrapStreamFn!(ctx)!;

    try {
      await wrapped({
        headers: { 'content-type': 'application/json' },
        body: '{"model":"auto","messages":[]}',
        url: `http://127.0.0.1:${port}/v1/chat/completions`,
      });
    } finally {
      server.close();
    }

    assert.ok(capturedParams !== null, 'inner stream function must be called');
    const captured2 = capturedParams as { headers: Record<string, string> };
    // In dev_bypass mode with flag set, payment-signature is NOT injected
    assert.ok(
      !('payment-signature' in captured2.headers),
      'payment-signature must NOT be injected in dev_bypass mode',
    );
  });
});

describe('resolveDynamicModel', () => {
  it('solvela/auto resolves to { id: "auto" }', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const result = cfg.resolveDynamicModel!({ modelId: 'solvela/auto' });
    assert.strictEqual(result?.id, 'auto');
  });

  it('direct model ID is passed through', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    const result = cfg.resolveDynamicModel!({ modelId: 'gpt-4o' });
    assert.strictEqual(result?.id, 'gpt-4o');
  });

  it('unknown solvela/ prefix throws with suggestion (HF-P3-H4)', () => {
    const { api, config } = makeMockApi();
    register(api);
    const cfg = (config as unknown as { config: ProviderConfig }).config;
    assert.throws(
      () => cfg.resolveDynamicModel!({ modelId: 'solvela/premum' }),
      (err: Error) => {
        assert.ok(err.message.includes("Unknown Solvela profile 'solvela/premum'"), `got: ${err.message}`);
        assert.ok(err.message.includes('Known profiles'), `got: ${err.message}`);
        return true;
      },
    );
  });
});

describe('GatewayAcceptedWithoutPayment', () => {
  it('is exported and correctly named', () => {
    const err = new GatewayAcceptedWithoutPayment();
    assert.strictEqual(err.name, 'GatewayAcceptedWithoutPayment');
    assert.ok(err instanceof Error);
  });
});
