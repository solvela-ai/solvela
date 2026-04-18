/**
 * Public wallet adapter interface for Solvela AI SDK provider.
 *
 * Every signer is an adapter — raw private keys are not accepted at this
 * boundary. Production users implement their own adapter backed by a hardware
 * wallet, MPC signer, or wallet-standard adapter.
 *
 * For dev/test use only, import `createLocalWalletAdapter` from
 * `@solvela/ai-sdk-provider/adapters/local`.
 *
 * NOTE: Phase 3 fetch-wrapper applies §4.3 ALLOWED_402_FIELDS allowlist on
 * responseBody surfacing — the fields below represent the full wire type,
 * not the allowlisted subset emitted in error objects.
 */

/**
 * A single payment option in the 402 `accepts[]` array.
 * Shape matches the fixture at tests/fixtures/402-envelope.json.
 */
export interface SolvelaPaymentAccept {
  /** Payment scheme. v1 only supports "exact". */
  scheme: string;
  /** Solana network identifier (e.g. "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp" for mainnet). */
  network: string;
  /** Amount in USDC atomic units (e.g. "2625" = 0.002625 USDC). */
  amount: string;
  /** USDC mint address. Mainnet: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v". */
  asset: string;
  /** Recipient wallet public key. */
  pay_to: string;
  /** Maximum seconds the payment is valid. */
  max_timeout_seconds: number;
}

/**
 * Cost breakdown included in the 402 payment-required body.
 */
export interface SolvelaPaymentCostBreakdown {
  /** Provider cost as decimal string. */
  provider_cost: string;
  /** Platform fee as decimal string (5%). */
  platform_fee: string;
  /** Total cost as decimal string. */
  total: string;
  /** Currency symbol, always "USDC" in v1. */
  currency: string;
  /** Platform fee percentage (always 5). */
  fee_percent: number;
}

/**
 * The parsed payment-required payload that the wallet adapter receives.
 * Derived from the JSON-stringified value in the gateway 402 envelope's
 * `error.message` field.
 *
 * Shape matches `tests/fixtures/402-envelope.json` exactly.
 */
export interface SolvelaPaymentRequired {
  /** Protocol version (currently 2). */
  x402_version: number;
  /** The resource that triggered the payment requirement. */
  resource: {
    url: string;
    method: string;
  };
  /** Ordered list of accepted payment options. v1 selects the first
   *  entry with scheme=exact and asset=USDC. */
  accepts: SolvelaPaymentAccept[];
  /** Human-readable cost breakdown. */
  cost_breakdown: SolvelaPaymentCostBreakdown;
  /** Human-readable error message from the gateway. */
  error: string;
}

/**
 * Adapter interface that every signer must implement.
 *
 * Matches the typed-adapter pattern used by Coinbase x402, Solana Wallet
 * Adapter, AWS SigV4, and wagmi/viem. No raw key bytes are accepted at this
 * boundary.
 */
export interface SolvelaWalletAdapter {
  /**
   * Adapter identity used for logs and metrics.
   * Examples: "local-test-keypair", "phantom", "ledger", "mpc-signer".
   */
  readonly label: string;

  /**
   * Sign a parsed 402 payment-required and return the base64-encoded
   * PAYMENT-SIGNATURE header value.
   *
   * @param args.paymentRequired - The parsed 402 payload.
   * @param args.resourceUrl     - The URL of the resource being requested.
   * @param args.requestBody     - The JSON-stringified request body (capped at
   *                               maxBodyBytes before this is called).
   * @param args.signal          - Optional AbortSignal for cancellation.
   * @returns Promise resolving to the base64 PAYMENT-SIGNATURE header value.
   */
  signPayment(args: {
    paymentRequired: SolvelaPaymentRequired;
    resourceUrl: string;
    requestBody: string;
    signal?: AbortSignal;
  }): Promise<string>;
}
