import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { fetchHealth, fetchPricing, fetchModels } from "@/lib/api";

// ─── Helpers ──────────────────────────────────────────────────────────────────

function mockFetch(status: number, body: unknown) {
  return vi.fn().mockResolvedValue({
    ok: status >= 200 && status < 300,
    status,
    json: () => Promise.resolve(body),
  });
}

// ─── fetchHealth ──────────────────────────────────────────────────────────────

describe("fetchHealth", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("returns parsed health response on success", async () => {
    const payload = { status: "ok", version: "0.1.0" };
    vi.stubGlobal("fetch", mockFetch(200, payload));

    const result = await fetchHealth();

    expect(result).toEqual(payload);
  });

  it("calls the /health endpoint", async () => {
    const fetchSpy = mockFetch(200, { status: "ok", version: "0.1.0" });
    vi.stubGlobal("fetch", fetchSpy);

    await fetchHealth();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/health");
  });

  it("throws when response is not ok", async () => {
    vi.stubGlobal("fetch", mockFetch(503, {}));

    await expect(fetchHealth()).rejects.toThrow("Health check failed: 503");
  });

  it("throws on 404", async () => {
    vi.stubGlobal("fetch", mockFetch(404, {}));

    await expect(fetchHealth()).rejects.toThrow("Health check failed: 404");
  });
});

// ─── fetchPricing ─────────────────────────────────────────────────────────────

describe("fetchPricing", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const pricingPayload = {
    platform: {
      name: "RustyClawRouter",
      chain: "solana",
      token: "USDC",
      usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
      fee_percent: 5,
      settlement: "instant",
    },
    models: [
      {
        id: "gpt-4o-mini",
        display_name: "GPT-4o Mini",
        provider: "openai",
        pricing: {
          input_per_million_usdc: 0.15,
          output_per_million_usdc: 0.6,
          platform_fee_percent: 5,
          currency: "USDC",
        },
        capabilities: {
          streaming: true,
          tools: true,
          vision: true,
          reasoning: false,
          context_window: 128000,
        },
        example_1k_token_request: {
          input_tokens: 800,
          output_tokens: 200,
          provider_cost_usdc: "0.000234",
          platform_fee_usdc: "0.000012",
          total_usdc: "0.000246",
        },
      },
    ],
  };

  it("returns parsed pricing response on success", async () => {
    vi.stubGlobal("fetch", mockFetch(200, pricingPayload));

    const result = await fetchPricing();

    expect(result.platform.name).toBe("RustyClawRouter");
    expect(result.models).toHaveLength(1);
    expect(result.models[0].id).toBe("gpt-4o-mini");
  });

  it("calls the /pricing endpoint", async () => {
    const fetchSpy = mockFetch(200, pricingPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchPricing();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/pricing");
  });

  it("throws when response is not ok", async () => {
    vi.stubGlobal("fetch", mockFetch(500, {}));

    await expect(fetchPricing()).rejects.toThrow("Pricing fetch failed: 500");
  });
});

// ─── fetchModels ──────────────────────────────────────────────────────────────

describe("fetchModels", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const modelsPayload = {
    data: [
      {
        id: "gpt-4o-mini",
        display_name: "GPT-4o Mini",
        provider: "openai",
        pricing: {
          input_per_million_usdc: 0.15,
          output_per_million_usdc: 0.6,
          platform_fee_percent: 5,
          currency: "USDC",
        },
        capabilities: {
          streaming: true,
          tools: true,
          vision: true,
          reasoning: false,
          context_window: 128000,
        },
        example_1k_token_request: {
          input_tokens: 800,
          output_tokens: 200,
          provider_cost_usdc: "0.000234",
          platform_fee_usdc: "0.000012",
          total_usdc: "0.000246",
        },
      },
    ],
  };

  it("returns model list on success", async () => {
    vi.stubGlobal("fetch", mockFetch(200, modelsPayload));

    const result = await fetchModels();

    expect(result.data).toHaveLength(1);
    expect(result.data[0].id).toBe("gpt-4o-mini");
  });

  it("calls the /v1/models endpoint", async () => {
    const fetchSpy = mockFetch(200, modelsPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchModels();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/v1/models");
  });

  it("throws when response is not ok", async () => {
    vi.stubGlobal("fetch", mockFetch(401, {}));

    await expect(fetchModels()).rejects.toThrow("Models fetch failed: 401");
  });
});
