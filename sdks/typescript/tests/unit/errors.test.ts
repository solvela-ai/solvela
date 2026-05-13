import { describe, it, expect } from 'vitest';
import {
  ClientError,
  WalletError,
  SignerError,
  InsufficientBalanceError,
  GatewayError,
  PaymentRequiredError,
  PaymentRejectedError,
  RecipientMismatchError,
  AmountExceedsMaxError,
  TimeoutError,
} from '../../src/errors.js';
import { PaymentRequired, PaymentAccept, Resource, CostBreakdown } from '../../src/types.js';

describe('ClientError', () => {
  it('is an instance of Error', () => {
    const err = new ClientError('test');
    expect(err).toBeInstanceOf(Error);
    expect(err).toBeInstanceOf(ClientError);
    expect(err.name).toBe('ClientError');
    expect(err.message).toBe('test');
  });
});

describe('WalletError', () => {
  it('inherits from ClientError', () => {
    const err = new WalletError('bad wallet');
    expect(err).toBeInstanceOf(ClientError);
    expect(err.name).toBe('WalletError');
  });
});

describe('SignerError', () => {
  it('inherits from ClientError', () => {
    const err = new SignerError('sign failed');
    expect(err).toBeInstanceOf(ClientError);
    expect(err.name).toBe('SignerError');
  });
});

describe('InsufficientBalanceError', () => {
  it('has have and need attributes', () => {
    const err = new InsufficientBalanceError(100, 500);
    expect(err).toBeInstanceOf(ClientError);
    expect(err.have).toBe(100);
    expect(err.need).toBe(500);
    expect(err.message).toContain('100');
    expect(err.message).toContain('500');
  });
});

describe('GatewayError', () => {
  it('has status and message', () => {
    const err = new GatewayError(500, 'Internal error');
    expect(err).toBeInstanceOf(ClientError);
    expect(err.status).toBe(500);
    expect(err.message).toBe('Internal error');
  });
});

describe('PaymentRequiredError', () => {
  it('holds PaymentRequired data', () => {
    const pr = new PaymentRequired(
      2,
      new Resource('/v1/chat/completions', 'POST'),
      [],
      new CostBreakdown('0', '0', '0', 'USDC', 5),
      'Payment required',
    );
    const err = new PaymentRequiredError(pr);
    expect(err).toBeInstanceOf(ClientError);
    expect(err.paymentRequired.x402Version).toBe(2);
  });
});

describe('PaymentRejectedError', () => {
  it('inherits from ClientError', () => {
    const err = new PaymentRejectedError('rejected');
    expect(err).toBeInstanceOf(ClientError);
    expect(err.name).toBe('PaymentRejectedError');
  });
});

describe('RecipientMismatchError', () => {
  it('has expected and actual', () => {
    const err = new RecipientMismatchError('addr1', 'addr2');
    expect(err).toBeInstanceOf(ClientError);
    expect(err.expected).toBe('addr1');
    expect(err.actual).toBe('addr2');
    expect(err.message).toContain('addr1');
    expect(err.message).toContain('addr2');
  });
});

describe('AmountExceedsMaxError', () => {
  it('has amount and maxAmount', () => {
    const err = new AmountExceedsMaxError(1000, 500);
    expect(err).toBeInstanceOf(ClientError);
    expect(err.amount).toBe(1000);
    expect(err.maxAmount).toBe(500);
  });
});

describe('TimeoutError', () => {
  it('has timeoutSecs', () => {
    const err = new TimeoutError(30);
    expect(err).toBeInstanceOf(ClientError);
    expect(err.timeoutSecs).toBe(30);
    expect(err.message).toContain('30');
  });
});
