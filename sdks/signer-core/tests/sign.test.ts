/**
 * Tests for the x402 PAYMENT-SIGNATURE builder.
 *
 * Covers stub-mode (no privateKey) and protocol shape. Real-signing
 * paths (privateKey supplied) require Solana RPC and are not exercised
 * here — `tests/sign-live.test.ts` is the place for that, gated behind
 * `SOLVELA_LIVE_TESTS=1` (not yet committed).
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';

import {
  createPaymentHeader,
  decodePaymentHeader,
  SigningError,
} from '../src/sign.ts';
import type { PaymentRequired } from '../src/types.ts';

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

const directPaymentRequired: PaymentRequired = {
  x402_version: 2,
  accepts: [
    {
      scheme: 'exact',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2625',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: 'RCRgateway111111111111111111111111111111111',
      max_timeout_seconds: 300,
    },
  ],
  cost_breakdown: {
    provider_cost: '0.002500',
    platform_fee: '0.000125',
    total: '0.002625',
    currency: 'USDC',
    fee_percent: 5,
  },
  error: 'Payment required',
};

const escrowPaymentRequired: PaymentRequired = {
  x402_version: 2,
  accepts: [
    {
      scheme: 'escrow',
      network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      amount: '2625',
      asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      pay_to: 'RCRgateway111111111111111111111111111111111',
      max_timeout_seconds: 300,
      escrow_program_id: 'RCRescrow1111111111111111111111111111111111',
    },
  ],
  cost_breakdown: {
    provider_cost: '0.002500',
    platform_fee: '0.000125',
    total: '0.002625',
    currency: 'USDC',
    fee_percent: 5,
  },
  error: 'Payment required',
};

const RESOURCE_URL = 'http://localhost:8402/v1/chat/completions';

// ---------------------------------------------------------------------------
// Header construction (direct / exact scheme)
// ---------------------------------------------------------------------------

describe('createPaymentHeader (direct, stub mode)', () => {
  it('produces a base64 string', async () => {
    const header = await createPaymentHeader(directPaymentRequired, RESOURCE_URL);
    assert.match(header, /^[A-Za-z0-9+/=]+$/);
  });

  it('roundtrips through decode with full gateway envelope', async () => {
    const header = await createPaymentHeader(directPaymentRequired, RESOURCE_URL);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;

    // Top-level wire fields match crates/protocol/src/payment.rs:75-81 PaymentPayload.
    assert.equal(decoded.x402_version, 2);
    assert.deepEqual(decoded.resource, { url: RESOURCE_URL, method: 'POST' });
    assert.deepEqual(decoded.accepted, directPaymentRequired.accepts[0]);
    assert.deepEqual(decoded.payload, { transaction: 'STUB_BASE64_TX' });
  });

  it('throws on empty accepts array', async () => {
    const empty: PaymentRequired = { ...directPaymentRequired, accepts: [] };
    await assert.rejects(
      () => createPaymentHeader(empty, RESOURCE_URL),
      /No payment accept options/,
    );
  });

  it('falls back to first accept when escrow accept lacks escrow_program_id', async () => {
    const mixed: PaymentRequired = {
      ...escrowPaymentRequired,
      accepts: [
        {
          // escrow scheme but no program_id — should NOT be selected by the
          // escrow-finder; the first accept is taken and the direct path
          // (which expects `transaction`) is used.
          scheme: 'escrow',
          network: 'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
          amount: '2625',
          asset: 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
          pay_to: 'RCRgateway111111111111111111111111111111111',
          max_timeout_seconds: 300,
        },
      ],
    };
    const header = await createPaymentHeader(mixed, RESOURCE_URL);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    assert.deepEqual(decoded.payload, { transaction: 'STUB_BASE64_TX' });
  });
});

// ---------------------------------------------------------------------------
// Header construction (escrow scheme)
// ---------------------------------------------------------------------------

describe('createPaymentHeader (escrow, stub mode)', () => {
  it('produces an envelope with deposit_tx, service_id, agent_pubkey', async () => {
    const header = await createPaymentHeader(
      escrowPaymentRequired,
      RESOURCE_URL,
      undefined,
      '{"model":"gpt-4o"}',
    );
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;

    assert.equal(decoded.x402_version, 2);
    assert.deepEqual(decoded.resource, { url: RESOURCE_URL, method: 'POST' });

    const accepted = decoded.accepted as Record<string, unknown>;
    assert.equal(accepted.scheme, 'escrow');
    assert.equal(
      accepted.escrow_program_id,
      'RCRescrow1111111111111111111111111111111111',
    );

    const payload = decoded.payload as Record<string, unknown>;
    assert.equal(payload.deposit_tx, 'STUB_ESCROW_DEPOSIT_TX');
    assert.equal(payload.agent_pubkey, 'STUB_AGENT_PUBKEY');
    assert.equal(typeof payload.service_id, 'string');
    assert.ok((payload.service_id as string).length > 0);
  });

  it('service_id differs across calls (random component)', async () => {
    const body = '{"model":"gpt-4o"}';
    const h1 = await createPaymentHeader(escrowPaymentRequired, RESOURCE_URL, undefined, body);
    const h2 = await createPaymentHeader(escrowPaymentRequired, RESOURCE_URL, undefined, body);
    const p1 = (decodePaymentHeader(h1) as Record<string, unknown>).payload as Record<string, unknown>;
    const p2 = (decodePaymentHeader(h2) as Record<string, unknown>).payload as Record<string, unknown>;
    assert.notEqual(p1.service_id, p2.service_id);
  });

  it('service_id is base64 sha256 (44 chars including padding)', async () => {
    const header = await createPaymentHeader(
      escrowPaymentRequired,
      RESOURCE_URL,
      undefined,
      '{}',
    );
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    const payload = decoded.payload as Record<string, unknown>;
    const serviceId = payload.service_id as string;
    // 32 bytes -> 44 base64 chars with padding
    assert.equal(serviceId.length, 44);
    assert.match(serviceId, /^[A-Za-z0-9+/]+={0,2}$/);
  });
});

// ---------------------------------------------------------------------------
// SigningError shape
// ---------------------------------------------------------------------------

describe('SigningError', () => {
  it('extends Error and has name SigningError', () => {
    const err = new SigningError('boom');
    assert.ok(err instanceof Error);
    assert.equal(err.name, 'SigningError');
    assert.equal(err.message, 'boom');
  });

  it('does not expose a cause field — wrapping message is the safe surface', () => {
    // Cause is intentionally NOT preserved on SigningError. If a caller
    // serialised the error (JSON.stringify, structured logger), no
    // underlying buffer or stack from web3.js/spl-token/bs58 should leak.
    const err = new SigningError('wrapper');
    // No public cause property — `unknown` cast to satisfy strict TS.
    assert.equal((err as unknown as { cause?: unknown }).cause, undefined);
  });
});

// ---------------------------------------------------------------------------
// Wire format compat with crates/protocol/src/payment.rs
// ---------------------------------------------------------------------------

describe('Wire format', () => {
  it('produces top-level keys the Rust gateway expects', async () => {
    // The Rust PaymentPayload struct (crates/protocol/src/payment.rs:75-81)
    // requires exactly these top-level fields. If this test ever fails,
    // the gateway's serde_json::from_str::<PaymentPayload>() will reject
    // every header we send — wire-incompat regression sentinel.
    const header = await createPaymentHeader(directPaymentRequired, RESOURCE_URL);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    const keys = Object.keys(decoded).sort();
    assert.deepEqual(keys, ['accepted', 'payload', 'resource', 'x402_version']);
  });

  it('escrow inner payload has exactly deposit_tx, service_id, agent_pubkey', async () => {
    // PayloadData::Escrow variant requires these three fields and only these
    // three. Extra fields fail untagged-enum dispatch on the gateway side.
    const header = await createPaymentHeader(
      escrowPaymentRequired,
      RESOURCE_URL,
      undefined,
      '',
    );
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    const payload = decoded.payload as Record<string, unknown>;
    const keys = Object.keys(payload).sort();
    assert.deepEqual(keys, ['agent_pubkey', 'deposit_tx', 'service_id']);
  });

  it('direct inner payload has exactly transaction', async () => {
    const header = await createPaymentHeader(directPaymentRequired, RESOURCE_URL);
    const decoded = decodePaymentHeader(header) as Record<string, unknown>;
    const payload = decoded.payload as Record<string, unknown>;
    const keys = Object.keys(payload).sort();
    assert.deepEqual(keys, ['transaction']);
  });
});
