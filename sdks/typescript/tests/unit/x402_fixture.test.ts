// Cross-SDK 402-parse smoke (channel plan PR-B, invariant 12 / HALT 12).
//
// Parses the LIVE gateway 402 challenge fixture shared with the Go and Python
// SDK suites (`crates/gateway/tests/fixtures/chat_402_challenge.json`, pinned
// byte-shape-identical to the real route by the gateway integration test
// `x402_challenge_smoke_tests::live_402_body_matches_cross_sdk_fixture`).
//
// This SDK's `parseScheme` rejects the ENTIRE 402 on any unknown scheme, so a
// gateway that ever advertised a new scheme (e.g. `channel`) in `accepts[]`
// would break every deployed paid call at parse time — the memorialized
// cross-repo wire-drift failure mode. This smoke is the standing tripwire:
// the live shape must keep parsing, and the strictness itself is pinned.
import { readFileSync } from 'node:fs';
import { describe, it, expect } from 'vitest';

import { PaymentRequired, PaymentAccept } from '../../src/types.js';
import { ClientError } from '../../src/errors.js';

const FIXTURE_URL = new URL(
  '../../../../crates/gateway/tests/fixtures/chat_402_challenge.json',
  import.meta.url,
);

describe('live 402 challenge fixture (cross-SDK smoke)', () => {
  it('parses the live gateway 402 body', () => {
    const raw = JSON.parse(readFileSync(FIXTURE_URL, 'utf-8'));
    const pr = PaymentRequired.fromJSON(raw);
    expect(pr.accepts.length).toBeGreaterThan(0);
    for (const accept of pr.accepts) {
      expect(['exact', 'escrow']).toContain(accept.scheme);
    }
    expect(pr.resource.url).toBe('/v1/chat/completions');
    expect(pr.costBreakdown.currency).toBe('USDC');
  });

  it('rejects an accepts[] entry carrying an unknown scheme (why the gateway must never advertise one)', () => {
    const raw = JSON.parse(readFileSync(FIXTURE_URL, 'utf-8'));
    const channelEntry = { ...raw.accepts[0], scheme: 'channel' };
    expect(() => PaymentAccept.fromJSON(channelEntry)).toThrow(ClientError);
    // ...and the rejection takes down the WHOLE challenge parse — exact users
    // included — which is exactly the invariant-12 blast radius.
    expect(() =>
      PaymentRequired.fromJSON({ ...raw, accepts: [...raw.accepts, channelEntry] }),
    ).toThrow(ClientError);
  });
});
