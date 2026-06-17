// Browser → gateway client for `POST /v1/escrow/deposit-tx`.
//
// Calls the gateway directly from the dashboard client component. The gateway
// holds NO private key on this path — it returns an UNSIGNED message that the
// connected wallet signs. The response here is UNTRUSTED: callers MUST run it
// through `verifyDepositMessage` (verify.ts) before requesting a signature.
//
// Error envelopes differ across gateway code paths, so this maps BOTH shapes the
// gateway emits:
//   - `{ "error": { "type": "...", "message": "..." } }`  (GatewayError + 429)
//   - `{ "error": "escrow not configured" }`              (bare-string 404)
// into a single typed `EscrowDepositError` carrying the HTTP status.

import type { DepositTxRequestBody, DepositTxResponse } from "@/lib/escrow/types";

/**
 * Typed error for the deposit-tx request. `status` is the HTTP status (or `0`
 * for a network-level failure before any response).
 */
export class EscrowDepositError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.name = "EscrowDepositError";
    this.status = status;
  }
}

/** Join a gateway base URL with the endpoint path without doubling the slash. */
function endpointUrl(gatewayUrl: string): string {
  return `${gatewayUrl.replace(/\/+$/, "")}/v1/escrow/deposit-tx`;
}

/** Extract a human message from either gateway error-envelope shape. */
function messageFromErrorBody(body: unknown, status: number): string {
  if (body && typeof body === "object" && "error" in body) {
    const err = (body as { error: unknown }).error;
    if (typeof err === "string" && err.length > 0) {
      return err;
    }
    if (err && typeof err === "object" && "message" in err) {
      const msg = (err as { message: unknown }).message;
      if (typeof msg === "string" && msg.length > 0) {
        return msg;
      }
    }
  }
  return `escrow deposit request failed (HTTP ${status})`;
}

/**
 * Shape-check an untrusted 200 body before handing it to callers. This is NOT
 * the fund-safety gate (that is `verifyDepositMessage`); it only guarantees the
 * fields exist and are strings/numbers so downstream verification does not
 * dereference `undefined`. A missing field means the gateway returned something
 * we do not understand — fail closed.
 */
function assertDepositTxResponse(body: unknown): asserts body is DepositTxResponse {
  if (!body || typeof body !== "object") {
    throw new EscrowDepositError(200, "gateway returned a non-object response");
  }
  const b = body as Record<string, unknown>;
  if (typeof b.message !== "string" || typeof b.network !== "string") {
    throw new EscrowDepositError(200, "gateway response missing message/network");
  }
  const di = b.decoded_intent;
  if (!di || typeof di !== "object") {
    throw new EscrowDepositError(200, "gateway response missing decoded_intent");
  }
  const d = di as Record<string, unknown>;
  const strFields = [
    "program_id",
    "usdc_mint",
    "provider",
    "escrow_pda",
    "vault_ata",
    "amount",
    "service_id",
    "recent_blockhash",
  ];
  for (const f of strFields) {
    if (typeof d[f] !== "string") {
      throw new EscrowDepositError(200, `gateway decoded_intent missing field: ${f}`);
    }
  }
  // expiry_slot is a Solana u64 on the wire. A JS `number` cannot faithfully
  // hold a u64 > 2^53, so a non-safe-integer value would silently round and then
  // be compared against a rounded re-derivation in verify.ts. Reject rather than
  // trust a lossy value (fail-closed — never sign against a rounded expiry).
  if (typeof d.expiry_slot !== "number" || !Number.isSafeInteger(d.expiry_slot)) {
    throw new EscrowDepositError(200, "gateway decoded_intent missing field: expiry_slot");
  }
}

/**
 * Request an unsigned escrow-deposit message from the gateway.
 *
 * @param gatewayUrl base gateway URL (e.g. `NEXT_PUBLIC_GATEWAY_URL`).
 * @param body the deposit request (atomic-unit amount string — see amount.ts).
 * @throws {EscrowDepositError} on any non-2xx response or network failure.
 */
export async function requestDepositTx(
  gatewayUrl: string,
  body: DepositTxRequestBody,
): Promise<DepositTxResponse> {
  let res: Response;
  try {
    res = await fetch(endpointUrl(gatewayUrl), {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
      // Defense-in-depth: never silently follow a cross-origin redirect with the
      // request body. A misconfigured/hostile gateway redirect can't bypass the
      // verify gate, but we don't forward the POST off-origin either.
      redirect: "error",
    });
  } catch (err) {
    // Network-level failure (DNS, CORS, offline) — no HTTP status available.
    throw new EscrowDepositError(
      0,
      err instanceof Error ? err.message : "network error reaching the gateway",
    );
  }

  if (!res.ok) {
    let parsed: unknown;
    try {
      parsed = await res.json();
    } catch {
      // Body was not JSON — fall back to a status-only message.
      parsed = undefined;
    }
    throw new EscrowDepositError(res.status, messageFromErrorBody(parsed, res.status));
  }

  let data: unknown;
  try {
    data = await res.json();
  } catch {
    throw new EscrowDepositError(res.status, "gateway returned a non-JSON success body");
  }
  assertDepositTxResponse(data);
  return data;
}
