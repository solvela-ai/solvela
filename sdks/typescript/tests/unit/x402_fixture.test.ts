// Cross-SDK 402-parse smoke (channel plan PR-B / Slice B).
//
// Parses the LIVE gateway 402 challenge fixture shared with the Go and Python
// SDK suites (`crates/gateway/tests/fixtures/chat_402_challenge.json`, pinned
// byte-shape-identical to the real route by the gateway integration test
// `x402_challenge_smoke_tests::live_402_body_matches_cross_sdk_fixture`).
//
// Since Slice B (channel plan §7), `PaymentRequired.fromJSON` SKIPS
// unknown-scheme entries (representing them in `unrecognizedSchemes`) instead
// of rejecting the whole 402 — a gateway advertising a new scheme (e.g.
// `channel`) in `accepts[]` no longer breaks deployed paid calls at parse
// time. The direct per-entry parser (`PaymentAccept.fromJSON`) stays strict,
// and scheme selection stays fail-closed (skip is never select). This smoke
// pins that tolerance against the live fixture shape.
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

  it('tolerates an accepts[] entry carrying an unknown scheme (skip-or-represent, never select)', () => {
    const raw = JSON.parse(readFileSync(FIXTURE_URL, 'utf-8'));
    const channelEntry = { ...raw.accepts[0], scheme: 'channel' };
    // The DIRECT per-entry parser stays strict...
    expect(() => PaymentAccept.fromJSON(channelEntry)).toThrow(ClientError);
    // ...but the 402-body parse no longer poisons: the unknown entry is
    // skipped-and-represented, and the known entries survive unchanged for
    // fail-closed selection (Slice B, channel plan §7).
    const pr = PaymentRequired.fromJSON({
      ...raw,
      accepts: [...raw.accepts, channelEntry],
    });
    expect(pr.accepts.map((a) => a.scheme)).toEqual(
      raw.accepts.map((a: { scheme: string }) => a.scheme),
    );
    expect(pr.unrecognizedSchemes).toEqual(['channel']);
  });
});
