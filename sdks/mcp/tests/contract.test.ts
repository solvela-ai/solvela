/**
 * Contract test for the gateway 402 envelope shape.
 *
 * Loads the static fixture from crates/gateway/tests/fixtures/402-envelope.json
 * (copied locally to tests/fixtures/ for self-containment) and verifies that
 * GatewayClient.parse402 produces a PaymentRequired object with all fields
 * that createPaymentHeader from @solvela/sdk expects.
 *
 * If the gateway's 402 response shape drifts (e.g. a field is renamed or removed),
 * this test fails before the next release — catching the break without requiring
 * a live gateway.
 */

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { join, dirname } from 'node:path';

import { GatewayClient } from '../src/client.ts';
import type { PaymentRequired } from '../src/client.ts';
import { parse402 } from '@solvela/signer-core';

const __filename = fileURLToPath(import.meta.url);
const __dirname = dirname(__filename);

// Load the fixture file (self-contained copy — do not use symlinks)
const fixturePath = join(__dirname, 'fixtures', '402-envelope.json');
const fixtureRaw = readFileSync(fixturePath, 'utf-8');
const fixtureBody = JSON.parse(fixtureRaw) as { error: { type: string; message: string } };

describe('402 envelope contract', () => {
  it('fixture file is valid JSON with expected structure', () => {
    assert.ok(fixtureBody.error, 'fixture must have .error');
    assert.ok(typeof fixtureBody.error.message === 'string', 'fixture.error.message must be a string');
  });

  it('parse402 correctly parses the gateway fixture envelope', () => {
    // parse402 is now a module-level function from @solvela/signer-core that
    // accepts the raw response text string (the parallel executor extracted it).
    const parsed = parse402(JSON.stringify(fixtureBody));

    assert.ok(parsed !== null, 'parse402 must return a non-null PaymentRequired');
    const payment = parsed as PaymentRequired;

    // x402_version — required by the signer
    assert.ok(
      typeof payment.x402_version === 'number',
      `x402_version must be a number, got: ${typeof payment.x402_version}`,
    );
    assert.equal(payment.x402_version, 2, 'x402_version must be 2');

    // accepts — createPaymentHeader iterates this array
    assert.ok(Array.isArray(payment.accepts), 'accepts must be an array');
    assert.ok(payment.accepts.length > 0, 'accepts must have at least one entry');

    const accept = payment.accepts[0];
    assert.ok(typeof accept.scheme === 'string', 'accept.scheme must be a string');
    assert.ok(typeof accept.network === 'string', 'accept.network must be a string');
    assert.ok(typeof accept.amount === 'string', 'accept.amount must be a string');
    assert.ok(typeof accept.asset === 'string', 'accept.asset must be a string');
    assert.ok(typeof accept.pay_to === 'string', 'accept.pay_to must be a string');
    assert.ok(typeof accept.max_timeout_seconds === 'number', 'accept.max_timeout_seconds must be a number');

    // cost_breakdown — used for budget enforcement
    assert.ok(payment.cost_breakdown, 'cost_breakdown must be present');
    assert.ok(typeof payment.cost_breakdown.total === 'string', 'cost_breakdown.total must be a string');
    assert.ok(
      !isNaN(parseFloat(payment.cost_breakdown.total)),
      `cost_breakdown.total must be parseable as a float, got: ${payment.cost_breakdown.total}`,
    );
    assert.ok(typeof payment.cost_breakdown.currency === 'string', 'cost_breakdown.currency must be a string');
    assert.ok(typeof payment.cost_breakdown.fee_percent === 'number', 'cost_breakdown.fee_percent must be a number');

    // error string — informational
    assert.ok(typeof payment.error === 'string', 'error must be a string');
  });

  it('fixture accept fields are consistent with USDC on Solana mainnet', () => {
    const msg = JSON.parse(fixtureBody.error.message) as PaymentRequired;
    const accept = msg.accepts[0];

    // Asset must be the USDC mainnet SPL mint
    assert.equal(
      accept.asset,
      'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v',
      'asset must be the USDC mainnet SPL mint address',
    );

    // Network must be Solana mainnet-beta
    assert.equal(
      accept.network,
      'solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp',
      'network must be Solana mainnet-beta chain ID',
    );

    // Amount must be a positive integer string (micro-USDC)
    const amountInt = parseInt(accept.amount, 10);
    assert.ok(amountInt > 0, `amount must be a positive integer, got: ${accept.amount}`);
  });
});
