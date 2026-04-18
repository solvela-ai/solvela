/**
 * Unit-5: Error class contract tests.
 *
 * Scope (plan §6 Phase 7 Unit-5):
 *  - APICallError.isInstance correctness for all five error classes
 *  - isRetryable values per class, including SolvelaUpstreamError explicit override
 *  - Sentinel-leak battery: JSON.stringify, message, stack, cause, responseHeaders,
 *    requestBodyValues, toString — none may contain the sentinel key or signature
 *  - SolvelaSigningError Sec-15/M3: cause.message base58 is double-redacted BEFORE
 *    sanitizeError runs
 *  - Cycle-guard (WeakMap cache): shared sub-object referenced by both cause and
 *    requestBodyValues is redacted on both paths
 */

import { APICallError } from '@ai-sdk/provider';
import { describe, expect, it } from 'vitest';

import {
  SolvelaBudgetExceededError,
  SolvelaInvalidConfigError,
  SolvelaPaymentError,
  SolvelaSigningError,
  SolvelaUpstreamError,
} from '../../src/errors.js';

// ---------------------------------------------------------------------------
// Sentinel constants
// ---------------------------------------------------------------------------

/**
 * A plausible 64-char base58 string (matches BASE58_RE: /[1-9A-HJ-NP-Za-km-z]{44,88}/g).
 * This must never appear in any error surface after construction.
 *
 * Character set verified: no 0, O, I, l characters (excluded from base58).
 */
const SENTINEL_KEY =
  '5J1F7GHaDCCnxsG9tJ9D4i1LSdSFNaGo7Kwvj4kCaY3BZQMxGWEjvBL8n4HHLpk';

/**
 * A plausible payment-signature header value (base64 format).
 * This must be stripped from responseHeaders via stripPaymentSignature.
 * Note: base64 uses `=` which is NOT in the base58 alphabet, so this string
 * does NOT trigger the base58 redaction regex — it relies on header stripping.
 */
const SENTINEL_SIG =
  'AQAAALONGBASE64SIGVALUENOTFROMAREALKEYPAIR+/ABC=';

/** URL used for all APICallError-based error constructors. */
const DUMMY_URL = 'https://gateway.solvela.ai/v1/chat/completions';

// ---------------------------------------------------------------------------
// Shared sentinel-leak assertion helper
// ---------------------------------------------------------------------------

/**
 * Asserts that `value` (serialized to a string) does not contain either sentinel.
 * Accepts string, JSON-serializable value, or an error instance.
 */
function assertNoSentinel(label: string, value: unknown): void {
  const str =
    typeof value === 'string'
      ? value
      : JSON.stringify(value) ?? '';

  expect(str, `${label} must not contain SENTINEL_KEY`).not.toContain(
    SENTINEL_KEY,
  );
  // The base64 sig should not leak either (from headers or any other path)
  expect(str, `${label} must not contain SENTINEL_SIG`).not.toContain(
    SENTINEL_SIG,
  );
}

/**
 * Runs the full 7-surface sentinel-leak battery on an error instance.
 *
 * Surfaces tested:
 *  1. err.message
 *  2. err.stack
 *  3. err.toString()
 *  4. JSON.stringify(err)
 *  5. err.cause (serialized)
 *  6. err.responseHeaders (if present on APICallError)
 *  7. err.requestBodyValues (if present on APICallError)
 */
function runSentinelBattery(err: Error): void {
  assertNoSentinel('err.message', err.message);
  assertNoSentinel('err.stack', err.stack ?? '');
  assertNoSentinel('err.toString()', err.toString());
  assertNoSentinel('JSON.stringify(err)', JSON.stringify(err));

  // cause
  const cause = (err as Record<string, unknown>)['cause'];
  if (cause !== undefined) {
    assertNoSentinel('err.cause', cause);
  }

  // APICallError-specific fields
  if (err instanceof APICallError) {
    const headers = err.responseHeaders;
    if (headers !== undefined) {
      // Each header value individually
      for (const [k, v] of Object.entries(headers)) {
        assertNoSentinel(`err.responseHeaders["${k}"]`, v);
      }
      // The PAYMENT-SIGNATURE key itself must be absent
      const headerKeys = Object.keys(headers).map((k) => k.toLowerCase());
      expect(
        headerKeys,
        'PAYMENT-SIGNATURE header must be stripped',
      ).not.toContain('payment-signature');
    }

    assertNoSentinel('err.requestBodyValues', err.requestBodyValues);
  }
}

// ---------------------------------------------------------------------------
// Shared constructor params with sentinel injected into every field
// ---------------------------------------------------------------------------

function makeBaseParams(extra?: Record<string, unknown>) {
  return {
    message: `Payment failed: ${SENTINEL_KEY}`,
    url: DUMMY_URL,
    requestBodyValues: { prompt: SENTINEL_KEY, nested: { key: SENTINEL_KEY } },
    statusCode: 402,
    responseHeaders: {
      'PAYMENT-SIGNATURE': SENTINEL_SIG,
      'X-Request-Id': 'req-abc123',
      'Content-Type': 'application/json',
    },
    responseBody: JSON.stringify({ error: SENTINEL_KEY }),
    cause: new Error(`Upstream threw: ${SENTINEL_KEY}`),
    ...extra,
  };
}

// ===========================================================================
// SolvelaPaymentError
// ===========================================================================

describe('SolvelaPaymentError', () => {
  it('is recognized by APICallError.isInstance', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(APICallError.isInstance(err)).toBe(true);
  });

  it('is recognized by SolvelaPaymentError.isInstance', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(SolvelaPaymentError.isInstance(err)).toBe(true);
  });

  it('SolvelaPaymentError.isInstance returns false for unrelated errors', () => {
    expect(SolvelaPaymentError.isInstance(new Error('plain'))).toBe(false);
    expect(SolvelaPaymentError.isInstance(null)).toBe(false);
    expect(SolvelaPaymentError.isInstance('string')).toBe(false);
  });

  it('isRetryable is false (payment errors require a new payment, never retry)', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(err.isRetryable).toBe(false);
  });

  it('name is SolvelaPaymentError', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(err.name).toBe('SolvelaPaymentError');
  });

  it('passes full sentinel-leak battery with sentinel in every input field', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    runSentinelBattery(err);
  });

  it('strips PAYMENT-SIGNATURE header (case-insensitive: lowercase key)', () => {
    const err = new SolvelaPaymentError({
      ...makeBaseParams(),
      responseHeaders: {
        'payment-signature': SENTINEL_SIG,
        'x-other': 'safe-value',
      },
    });
    const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
      k.toLowerCase(),
    );
    expect(keys).not.toContain('payment-signature');
    expect(err.responseHeaders?.['x-other']).toBe('safe-value');
  });

  it('strips PAYMENT-SIGNATURE header (mixed case)', () => {
    const err = new SolvelaPaymentError({
      ...makeBaseParams(),
      responseHeaders: {
        'Payment-Signature': SENTINEL_SIG,
        'X-Safe': 'ok',
      },
    });
    const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
      k.toLowerCase(),
    );
    expect(keys).not.toContain('payment-signature');
  });

  it('preserves non-sensitive response headers', () => {
    const err = new SolvelaPaymentError({
      ...makeBaseParams(),
      responseHeaders: {
        'PAYMENT-SIGNATURE': SENTINEL_SIG,
        'X-Request-Id': 'req-abc',
        'Content-Type': 'application/json',
      },
    });
    expect(err.responseHeaders?.['X-Request-Id']).toBe('req-abc');
    expect(err.responseHeaders?.['Content-Type']).toBe('application/json');
  });

  it('redacts sentinel in message field', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(err.message).not.toContain(SENTINEL_KEY);
    expect(err.message).toContain('[REDACTED]');
  });

  it('redacts sentinel in requestBodyValues string leaves', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    const serialized = JSON.stringify(err.requestBodyValues);
    expect(serialized).not.toContain(SENTINEL_KEY);
    expect(serialized).toContain('[REDACTED]');
  });

  it('redacts sentinel in responseBody', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(err.responseBody).not.toContain(SENTINEL_KEY);
    expect(err.responseBody).toContain('[REDACTED]');
  });
});

// ===========================================================================
// SolvelaBudgetExceededError
// ===========================================================================

describe('SolvelaBudgetExceededError', () => {
  it('is recognized by APICallError.isInstance', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    expect(APICallError.isInstance(err)).toBe(true);
  });

  it('is recognized by SolvelaBudgetExceededError.isInstance', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    expect(SolvelaBudgetExceededError.isInstance(err)).toBe(true);
  });

  it('SolvelaBudgetExceededError.isInstance returns false for unrelated errors', () => {
    expect(SolvelaBudgetExceededError.isInstance(new Error('plain'))).toBe(false);
    expect(SolvelaBudgetExceededError.isInstance(null)).toBe(false);
  });

  it('isRetryable is false (budget exceeded requires caller action, not retry)', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    expect(err.isRetryable).toBe(false);
  });

  it('name is SolvelaBudgetExceededError', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    expect(err.name).toBe('SolvelaBudgetExceededError');
  });

  it('passes full sentinel-leak battery with sentinel in every input field', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    runSentinelBattery(err);
  });

  it('strips PAYMENT-SIGNATURE from responseHeaders', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
      k.toLowerCase(),
    );
    expect(keys).not.toContain('payment-signature');
  });

  it('redacts sentinel in requestBodyValues nested leaves', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    const serialized = JSON.stringify(err.requestBodyValues);
    expect(serialized).not.toContain(SENTINEL_KEY);
  });
});

// ===========================================================================
// SolvelaSigningError
// ===========================================================================

describe('SolvelaSigningError', () => {
  it('is recognized by APICallError.isInstance', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    expect(APICallError.isInstance(err)).toBe(true);
  });

  it('is recognized by SolvelaSigningError.isInstance', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    expect(SolvelaSigningError.isInstance(err)).toBe(true);
  });

  it('SolvelaSigningError.isInstance returns false for unrelated errors', () => {
    expect(SolvelaSigningError.isInstance(new Error('plain'))).toBe(false);
    expect(SolvelaSigningError.isInstance(undefined)).toBe(false);
  });

  it('isRetryable is false (signing failures indicate key/config problem)', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    expect(err.isRetryable).toBe(false);
  });

  it('name is SolvelaSigningError', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    expect(err.name).toBe('SolvelaSigningError');
  });

  it('passes full sentinel-leak battery with sentinel in every input field', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    runSentinelBattery(err);
  });

  it(
    'Sec-15/M3: redacts base58 key embedded in cause.message BEFORE sanitizeError runs (double-redaction)',
    () => {
      // The cause message contains the sentinel key directly.
      // SolvelaSigningError must explicitly redact it via redactBase58(redactHex(...))
      // before passing to sanitizeError — verifying the constructor-level double-redaction.
      const cause = new Error(
        `Wallet signing failed: key=${SENTINEL_KEY} tx=abc`,
      );
      const err = new SolvelaSigningError({
        ...makeBaseParams(),
        cause,
      });

      // The original cause object must not be mutated (immutability rule)
      expect(cause.message).toContain(SENTINEL_KEY);

      // The error's stored cause must have the key redacted
      const causeOnErr = (err as Record<string, unknown>)['cause'] as
        | Record<string, unknown>
        | undefined;
      const causeMsg =
        causeOnErr != null && typeof causeOnErr['message'] === 'string'
          ? causeOnErr['message']
          : '';
      expect(causeMsg).not.toContain(SENTINEL_KEY);
      expect(causeMsg).toContain('[REDACTED]');
    },
  );

  it(
    'Sec-15/M3: cause.message with sentinel does not leak via JSON.stringify or toString',
    () => {
      const cause = new Error(`signing key: ${SENTINEL_KEY}`);
      const err = new SolvelaSigningError({ ...makeBaseParams(), cause });
      assertNoSentinel('JSON.stringify(err)', JSON.stringify(err));
      assertNoSentinel('err.toString()', err.toString());
    },
  );

  it('strips PAYMENT-SIGNATURE from responseHeaders', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
      k.toLowerCase(),
    );
    expect(keys).not.toContain('payment-signature');
  });
});

// ===========================================================================
// SolvelaUpstreamError
// ===========================================================================

describe('SolvelaUpstreamError', () => {
  it('is recognized by APICallError.isInstance', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    expect(APICallError.isInstance(err)).toBe(true);
  });

  it('is recognized by SolvelaUpstreamError.isInstance', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    expect(SolvelaUpstreamError.isInstance(err)).toBe(true);
  });

  it('SolvelaUpstreamError.isInstance returns false for unrelated errors', () => {
    expect(SolvelaUpstreamError.isInstance(new Error('plain'))).toBe(false);
    expect(SolvelaUpstreamError.isInstance(null)).toBe(false);
  });

  it('name is SolvelaUpstreamError', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    expect(err.name).toBe('SolvelaUpstreamError');
  });

  // isRetryable derivation from statusCode
  it('isRetryable is true when statusCode is null/undefined (network error)', () => {
    const err = new SolvelaUpstreamError({
      ...makeBaseParams(),
      statusCode: undefined,
    });
    expect(err.isRetryable).toBe(true);
  });

  it('isRetryable is true for 5xx status codes', () => {
    for (const code of [500, 502, 503, 504]) {
      const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: code }));
      expect(err.isRetryable, `statusCode=${code} should be retryable`).toBe(
        true,
      );
    }
  });

  it('isRetryable is true for 408 (request timeout)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 408 }));
    expect(err.isRetryable).toBe(true);
  });

  it('isRetryable is true for 409 (conflict)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 409 }));
    expect(err.isRetryable).toBe(true);
  });

  it('isRetryable is true for 429 (rate limited)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 429 }));
    expect(err.isRetryable).toBe(true);
  });

  it('isRetryable is false for 400 (bad request)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 400 }));
    expect(err.isRetryable).toBe(false);
  });

  it('isRetryable is false for 401 (unauthorized)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 401 }));
    expect(err.isRetryable).toBe(false);
  });

  it('isRetryable is false for 403 (forbidden)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 403 }));
    expect(err.isRetryable).toBe(false);
  });

  it('isRetryable is false for 404 (not found)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 404 }));
    expect(err.isRetryable).toBe(false);
  });

  it('isRetryable is false for 422 (unprocessable entity)', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 422 }));
    expect(err.isRetryable).toBe(false);
  });

  // Explicit isRetryable override (recent fix verification)
  it('explicit isRetryable:true overrides a non-retryable status code (e.g. 400)', () => {
    const err = new SolvelaUpstreamError({
      ...makeBaseParams({ statusCode: 400 }),
      isRetryable: true,
    });
    expect(err.isRetryable).toBe(true);
  });

  it('explicit isRetryable:false overrides a retryable status code (e.g. 500)', () => {
    const err = new SolvelaUpstreamError({
      ...makeBaseParams({ statusCode: 500 }),
      isRetryable: false,
    });
    expect(err.isRetryable).toBe(false);
  });

  it('explicit isRetryable:false overrides network error (statusCode undefined)', () => {
    const err = new SolvelaUpstreamError({
      ...makeBaseParams(),
      statusCode: undefined,
      isRetryable: false,
    });
    expect(err.isRetryable).toBe(false);
  });

  it('passes full sentinel-leak battery with sentinel in every input field', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    runSentinelBattery(err);
  });

  it('strips PAYMENT-SIGNATURE from responseHeaders', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
      k.toLowerCase(),
    );
    expect(keys).not.toContain('payment-signature');
  });

  it('redacts sentinel in responseBody', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    expect(err.responseBody).not.toContain(SENTINEL_KEY);
    expect(err.responseBody).toContain('[REDACTED]');
  });
});

// ===========================================================================
// SolvelaInvalidConfigError
// ===========================================================================

describe('SolvelaInvalidConfigError', () => {
  it('is NOT recognized by APICallError.isInstance (extends AISDKError, not APICallError)', () => {
    const err = new SolvelaInvalidConfigError({
      message: 'Missing wallet adapter',
    });
    expect(APICallError.isInstance(err)).toBe(false);
  });

  it('is recognized by SolvelaInvalidConfigError.isInstance', () => {
    const err = new SolvelaInvalidConfigError({
      message: 'Missing wallet adapter',
    });
    expect(SolvelaInvalidConfigError.isInstance(err)).toBe(true);
  });

  it('SolvelaInvalidConfigError.isInstance returns false for unrelated errors', () => {
    expect(SolvelaInvalidConfigError.isInstance(new Error('plain'))).toBe(
      false,
    );
    expect(SolvelaInvalidConfigError.isInstance(null)).toBe(false);
    expect(SolvelaInvalidConfigError.isInstance(42)).toBe(false);
  });

  it('name is SolvelaInvalidConfigError', () => {
    const err = new SolvelaInvalidConfigError({
      message: 'Missing wallet adapter',
    });
    expect(err.name).toBe('SolvelaInvalidConfigError');
  });

  it('is an instance of Error', () => {
    const err = new SolvelaInvalidConfigError({
      message: 'bad config',
    });
    expect(err).toBeInstanceOf(Error);
  });

  it('passes full sentinel-leak battery with sentinel in message and cause', () => {
    const err = new SolvelaInvalidConfigError({
      message: `Invalid config: wallet key=${SENTINEL_KEY}`,
      cause: new Error(`Bad key format: ${SENTINEL_KEY}`),
    });
    // message surface
    assertNoSentinel('err.message', err.message);
    assertNoSentinel('err.stack', err.stack ?? '');
    assertNoSentinel('err.toString()', err.toString());
    assertNoSentinel('JSON.stringify(err)', JSON.stringify(err));
    const cause = (err as Record<string, unknown>)['cause'];
    if (cause !== undefined) {
      assertNoSentinel('err.cause', cause);
    }
  });

  it('redacts sentinel in message', () => {
    const err = new SolvelaInvalidConfigError({
      message: `Config error: ${SENTINEL_KEY}`,
    });
    expect(err.message).not.toContain(SENTINEL_KEY);
    expect(err.message).toContain('[REDACTED]');
  });

  it('constructs without cause', () => {
    const err = new SolvelaInvalidConfigError({
      message: 'missing wallet adapter',
    });
    expect(err.message).toBe('missing wallet adapter');
  });
});

// ===========================================================================
// Cross-class isInstance isolation (negative cases)
// ===========================================================================

describe('isInstance cross-class isolation', () => {
  it('SolvelaPaymentError.isInstance rejects SolvelaBudgetExceededError', () => {
    const err = new SolvelaBudgetExceededError(makeBaseParams());
    expect(SolvelaPaymentError.isInstance(err)).toBe(false);
  });

  it('SolvelaBudgetExceededError.isInstance rejects SolvelaPaymentError', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(SolvelaBudgetExceededError.isInstance(err)).toBe(false);
  });

  it('SolvelaSigningError.isInstance rejects SolvelaUpstreamError', () => {
    const err = new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 }));
    expect(SolvelaSigningError.isInstance(err)).toBe(false);
  });

  it('SolvelaUpstreamError.isInstance rejects SolvelaSigningError', () => {
    const err = new SolvelaSigningError(makeBaseParams());
    expect(SolvelaUpstreamError.isInstance(err)).toBe(false);
  });

  it('SolvelaInvalidConfigError.isInstance rejects SolvelaPaymentError', () => {
    const err = new SolvelaPaymentError(makeBaseParams());
    expect(SolvelaInvalidConfigError.isInstance(err)).toBe(false);
  });
});

// ===========================================================================
// Cycle-guard test (HIGH fix verification: WeakMap shared sub-object cache)
// ===========================================================================

describe('cycle-guard: shared sub-object redacted on both cause and requestBodyValues paths', () => {
  it(
    'shared sub-object with sentinel is redacted consistently via WeakMap cache',
    () => {
      // Construct an object containing the sentinel, then share it across
      // BOTH cause and requestBodyValues. The WeakMap in sanitizeError/redactUnknown
      // must return the already-redacted copy for the second traversal rather than
      // the original un-redacted value.
      const sharedPayload: Record<string, unknown> = {
        key: SENTINEL_KEY,
        metadata: 'tx context',
      };

      const err = new SolvelaPaymentError({
        message: 'payment failed',
        url: DUMMY_URL,
        requestBodyValues: { payload: sharedPayload },
        statusCode: 402,
        responseHeaders: { 'PAYMENT-SIGNATURE': SENTINEL_SIG },
        cause: { shared: sharedPayload, extra: 'context' },
      });

      // Verify sentinel is absent from requestBodyValues path
      assertNoSentinel(
        'err.requestBodyValues (shared sub-object path)',
        err.requestBodyValues,
      );

      // Verify sentinel is absent from cause path
      const cause = (err as Record<string, unknown>)['cause'];
      assertNoSentinel('err.cause (shared sub-object path)', cause);

      // Verify the full JSON serialization is clean
      assertNoSentinel(
        'JSON.stringify(err) — full cycle-guard verification',
        JSON.stringify(err),
      );
    },
  );

  it(
    'shared sub-object with sentinel is redacted for SolvelaUpstreamError (cycle-guard)',
    () => {
      const sharedPayload: Record<string, unknown> = {
        signerOutput: SENTINEL_KEY,
      };

      const err = new SolvelaUpstreamError({
        message: 'upstream failed',
        url: DUMMY_URL,
        requestBodyValues: sharedPayload,
        statusCode: 500,
        cause: { ref: sharedPayload },
      });

      assertNoSentinel('err.requestBodyValues', err.requestBodyValues);
      const cause = (err as Record<string, unknown>)['cause'];
      assertNoSentinel('err.cause', cause);
    },
  );
});

// ===========================================================================
// Sentinel-leak battery for all five error classes (consolidated table)
// ===========================================================================

describe('sentinel-leak battery — all five error classes', () => {
  it('SolvelaPaymentError: full 7-surface battery', () => {
    runSentinelBattery(new SolvelaPaymentError(makeBaseParams()));
  });

  it('SolvelaBudgetExceededError: full 7-surface battery', () => {
    runSentinelBattery(new SolvelaBudgetExceededError(makeBaseParams()));
  });

  it('SolvelaSigningError: full 7-surface battery', () => {
    runSentinelBattery(new SolvelaSigningError(makeBaseParams()));
  });

  it('SolvelaUpstreamError: full 7-surface battery (5xx)', () => {
    runSentinelBattery(
      new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 })),
    );
  });

  it('SolvelaInvalidConfigError: 5-surface battery (no HTTP fields)', () => {
    const err = new SolvelaInvalidConfigError({
      message: `Config error: ${SENTINEL_KEY}`,
      cause: new Error(`nested cause: ${SENTINEL_KEY}`),
    });
    assertNoSentinel('err.message', err.message);
    assertNoSentinel('err.stack', err.stack ?? '');
    assertNoSentinel('err.toString()', err.toString());
    assertNoSentinel('JSON.stringify(err)', JSON.stringify(err));
    const cause = (err as Record<string, unknown>)['cause'];
    if (cause !== undefined) {
      assertNoSentinel('err.cause', cause);
    }
  });

  it('PAYMENT-SIGNATURE absent from responseHeaders across all APICallError subclasses', () => {
    const classes = [
      new SolvelaPaymentError(makeBaseParams()),
      new SolvelaBudgetExceededError(makeBaseParams()),
      new SolvelaSigningError(makeBaseParams()),
      new SolvelaUpstreamError(makeBaseParams({ statusCode: 500 })),
    ];
    for (const err of classes) {
      const keys = Object.keys(err.responseHeaders ?? {}).map((k) =>
        k.toLowerCase(),
      );
      expect(
        keys,
        `${err.name} must strip PAYMENT-SIGNATURE from headers`,
      ).not.toContain('payment-signature');
    }
  });
});
