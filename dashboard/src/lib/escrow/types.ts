// Shared wire types for the BYO-wallet escrow-funding money path.
//
// These mirror the gateway's `POST /v1/escrow/deposit-tx` contract (gateway
// `crates/gateway/src/routes/escrow.rs`, shipped as #597). The `amount` field
// on the wire is ALWAYS an atomic-unit integer string of micro-USDC (6 decimals),
// never a decimal-USDC string — see `amount.ts` for the conversion.
//
// Treat every value the gateway returns as UNTRUSTED until `verify.ts` has
// independently re-derived the deposit accounts and byte-matched the message
// against the connected wallet ("verify-what-you-sign").

/**
 * Request body for `POST /v1/escrow/deposit-tx`.
 *
 * - `agent_wallet`: base58 pubkey of the external signer (the connected wallet);
 *   becomes the fee payer / signer / escrow owner.
 * - `service_id`: base64 of the 32-byte service id seeding the escrow PDA.
 * - `amount`: atomic micro-USDC as a canonical integer string (e.g. `"2625"`),
 *   `> 0`. The gateway rejects non-canonical strings (leading zeros, sign,
 *   whitespace), so always pass the output of `usdcDecimalToAtomic`.
 * - `expiry_slot`: optional absolute expiry slot; omitted lets the gateway pick
 *   a default inside its window.
 */
export interface DepositTxRequestBody {
  agent_wallet: string;
  service_id: string;
  amount: string;
  expiry_slot?: number;
}

/**
 * The deterministic inputs the gateway claims the unsigned message was built
 * from. UNTRUSTED until verified: `verify.ts` re-derives every account here from
 * the connected wallet and confirms it matches both this intent AND the decoded
 * message bytes before any signature is requested.
 */
export interface DecodedIntent {
  program_id: string;
  usdc_mint: string;
  provider: string;
  escrow_pda: string;
  vault_ata: string;
  /** Atomic micro-USDC integer string (echoes the validated request amount). */
  amount: string;
  /** Base64 of the 32-byte service_id (echoes the validated request). */
  service_id: string;
  expiry_slot: number;
  /** Base58 recent blockhash embedded in the message. */
  recent_blockhash: string;
}

/**
 * Response body for `POST /v1/escrow/deposit-tx`.
 *
 * `message` is base64 of the UNSIGNED legacy message bytes ONLY (not a full
 * signed transaction): the signer signs these bytes and assembles
 * `compact-u16(1) || signature(64) || message` itself.
 */
export interface DepositTxResponse {
  message: string;
  decoded_intent: DecodedIntent;
  network: string;
}
