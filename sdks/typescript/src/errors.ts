import type { PaymentRequired } from './types.js';

export class ClientError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ClientError';
  }
}

export class WalletError extends ClientError {
  constructor(message: string) {
    super(message);
    this.name = 'WalletError';
  }
}

export class SignerError extends ClientError {
  constructor(message: string) {
    super(message);
    this.name = 'SignerError';
  }
}

export class InsufficientBalanceError extends ClientError {
  constructor(
    public readonly have: number,
    public readonly need: number,
  ) {
    super(`Insufficient balance: have ${have}, need ${need}`);
    this.name = 'InsufficientBalanceError';
  }
}

export class GatewayError extends ClientError {
  constructor(
    public readonly status: number,
    public override readonly message: string,
  ) {
    super(message);
    this.name = 'GatewayError';
  }
}

export class PaymentRequiredError extends ClientError {
  constructor(public readonly paymentRequired: PaymentRequired) {
    super(`Payment required: ${paymentRequired.error}`);
    this.name = 'PaymentRequiredError';
  }
}

/**
 * Raised when the gateway returns a 402 AFTER a signed payment was attached —
 * a post-signing rejection, distinct from the initial PaymentRequiredError
 * challenge. `paymentRequired` carries the second 402 body (cost breakdown,
 * accepted schemes, gateway error message) so callers can inspect WHY the
 * payment was rejected — replay nonce, expired signature, balance issue —
 * instead of a bare reason string. Optional for backward compatibility with
 * callers constructing this error from a message only.
 */
export class PaymentRejectedError extends ClientError {
  constructor(
    message: string,
    public readonly paymentRequired?: PaymentRequired,
  ) {
    super(message);
    this.name = 'PaymentRejectedError';
  }
}

export class RecipientMismatchError extends ClientError {
  constructor(
    public readonly expected: string,
    public readonly actual: string,
  ) {
    super(`Recipient mismatch: expected ${expected}, got ${actual}`);
    this.name = 'RecipientMismatchError';
  }
}

export class EscrowProgramMismatchError extends ClientError {
  constructor(
    public readonly expected: string,
    public readonly actual: string,
  ) {
    super(`Escrow program mismatch: expected ${expected}, got ${actual}`);
    this.name = 'EscrowProgramMismatchError';
  }
}

export class AmountExceedsMaxError extends ClientError {
  constructor(
    public readonly amount: number,
    public readonly maxAmount: number,
  ) {
    super(`Amount ${amount} exceeds maximum ${maxAmount}`);
    this.name = 'AmountExceedsMaxError';
  }
}

export class TimeoutError extends ClientError {
  constructor(public readonly timeoutSecs: number) {
    super(`Request timed out after ${timeoutSecs} seconds`);
    this.name = 'TimeoutError';
  }
}
