import { describe, it, expect, vi, afterEach } from "vitest";

import {
  requestDepositTx,
  EscrowDepositError,
} from "@/lib/escrow/deposit-client";
import type { DepositTxResponse } from "@/lib/escrow/types";

const GATEWAY = "http://localhost:8402";

const VALID_BODY = {
  agent_wallet: "2iXtA8oeZqUU5pofxK971TCEvFGfems2AcDRaZHKD2pQ",
  service_id: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
  amount: "2625",
};

const OK_RESPONSE: DepositTxResponse = {
  message: "AQAG...==",
  decoded_intent: {
    program_id: "9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU",
    usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    provider: "RecipientWallet111111111111111111111111111111",
    escrow_pda: "8itRK9Q7QErNubyjhuTjs85FHAfmQ7FzKjPRp6FBsaf1",
    vault_ata: "B3Gfznfz8k5ePdQgAqRkWCvL5U5Pg1jL8SbEoGKt7BXX",
    amount: "2625",
    service_id: "BwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwc=",
    expiry_slot: 1000750,
    recent_blockhash: "CZ8YUVdk7znjrUmnb5n7kgySk9yRAsQDYmyCxzfSky9t",
  },
  network: "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp",
};

function mockFetch(
  status: number,
  body: unknown,
  opts: { json?: boolean } = { json: true },
): typeof fetch {
  return vi.fn(async () =>
    new Response(opts.json !== false ? JSON.stringify(body) : String(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  ) as unknown as typeof fetch;
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("requestDepositTx", () => {
  it("POSTs to /v1/escrow/deposit-tx and returns the parsed response on 200", async () => {
    const fetchMock = mockFetch(200, OK_RESPONSE);
    vi.stubGlobal("fetch", fetchMock);

    const result = await requestDepositTx(GATEWAY, VALID_BODY);

    expect(result).toEqual(OK_RESPONSE);
    expect(fetchMock).toHaveBeenCalledOnce();
    const [url, init] = (fetchMock as unknown as ReturnType<typeof vi.fn>).mock
      .calls[0];
    expect(url).toBe(`${GATEWAY}/v1/escrow/deposit-tx`);
    expect(init.method).toBe("POST");
    expect(JSON.parse(init.body)).toEqual(VALID_BODY);
    expect(
      new Headers(init.headers).get("content-type"),
    ).toBe("application/json");
  });

  it("does not double up a trailing slash in the gateway URL", async () => {
    const fetchMock = mockFetch(200, OK_RESPONSE);
    vi.stubGlobal("fetch", fetchMock);

    await requestDepositTx(`${GATEWAY}/`, VALID_BODY);

    const [url] = (fetchMock as unknown as ReturnType<typeof vi.fn>).mock
      .calls[0];
    expect(url).toBe(`${GATEWAY}/v1/escrow/deposit-tx`);
  });

  it("maps a 404 escrow-not-configured ({error:string}) envelope", async () => {
    vi.stubGlobal(
      "fetch",
      mockFetch(404, { error: "escrow not configured" }),
    );

    await expect(requestDepositTx(GATEWAY, VALID_BODY)).rejects.toMatchObject({
      status: 404,
      message: expect.stringContaining("escrow not configured"),
    });
  });

  it("maps a 400 bad-request ({error:{message}}) envelope", async () => {
    vi.stubGlobal(
      "fetch",
      mockFetch(400, {
        error: { type: "bad_request", message: "amount must be greater than zero" },
      }),
    );

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.status).toBe(400);
    expect(err.message).toContain("amount must be greater than zero");
  });

  it("maps a 429 rate-limited envelope", async () => {
    vi.stubGlobal(
      "fetch",
      mockFetch(429, {
        error: { type: "rate_limit_exceeded", message: "Too many requests. Please slow down." },
      }),
    );

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.status).toBe(429);
    expect(err.message.toLowerCase()).toContain("too many requests");
  });

  it("maps a 503 RPC-unavailable envelope", async () => {
    vi.stubGlobal(
      "fetch",
      mockFetch(503, {
        error: {
          type: "service_unavailable",
          message: "could not reach the Solana cluster to build the deposit",
        },
      }),
    );

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.status).toBe(503);
    expect(err.message).toContain("Solana cluster");
  });

  it("maps a 500 with a fallback message when the body is not JSON", async () => {
    vi.stubGlobal(
      "fetch",
      mockFetch(500, "Internal Server Error", { json: false }),
    );

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.status).toBe(500);
    expect(err.message).toContain("500");
  });

  it("wraps a network failure as EscrowDepositError with status 0", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("Failed to fetch");
      }) as unknown as typeof fetch,
    );

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.status).toBe(0);
  });

  it("throws if the 200 body is missing required fields (untrusted response)", async () => {
    vi.stubGlobal("fetch", mockFetch(200, { network: "solana:x" }));

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
  });

  it("rejects a 200 body whose expiry_slot is not a safe integer (u64 > 2^53)", async () => {
    // A Solana u64 expiry beyond Number.MAX_SAFE_INTEGER cannot round-trip
    // through a JS number; trusting it would compare a rounded value downstream.
    // assertDepositTxResponse must fail closed.
    const body = {
      ...OK_RESPONSE,
      decoded_intent: {
        ...OK_RESPONSE.decoded_intent,
        expiry_slot: Number.MAX_SAFE_INTEGER + 1,
      },
    };
    vi.stubGlobal("fetch", mockFetch(200, body));

    const err = await requestDepositTx(GATEWAY, VALID_BODY).catch((e) => e);
    expect(err).toBeInstanceOf(EscrowDepositError);
    expect(err.message.toLowerCase()).toContain("expiry_slot");
  });
});
