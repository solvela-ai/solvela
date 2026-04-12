import { describe, it, expect, vi, afterEach } from "vitest";
import {
  fetchHealth,
  fetchPricing,
  fetchModels,
  fetchAdminStats,
  fetchServices,
  fetchEscrowConfig,
  fetchEscrowHealth,
} from "@/lib/api";

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
      name: "Solvela",
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

    expect(result.platform.name).toBe("Solvela");
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

// ─── fetchAdminStats ──────────────────────────────────────────────────────────

describe("fetchAdminStats", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    delete process.env.GATEWAY_ADMIN_KEY;
  });

  const statsPayload = {
    period_days: 30,
    summary: {
      total_requests: 1247,
      total_cost_usdc: "3.847291",
      total_input_tokens: 892400,
      total_output_tokens: 341200,
      unique_wallets: 12,
      cache_hit_rate: 0.23,
    },
    by_model: [
      {
        model: "anthropic/claude-sonnet-4-20250514",
        provider: "anthropic",
        requests: 412,
        cost_usdc: "1.923000",
        input_tokens: 310000,
        output_tokens: 142000,
      },
    ],
    by_day: [
      {
        date: "2026-03-11",
        requests: 47,
        cost_usdc: "0.142300",
        spend: 0.1423,
      },
    ],
    top_wallets: [
      {
        wallet: "7xKXtg...",
        requests: 200,
        cost_usdc: "0.843291",
      },
    ],
  };

  it("returns parsed stats on success", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    vi.stubGlobal("fetch", mockFetch(200, statsPayload));

    const result = await fetchAdminStats(30);

    expect(result).not.toBeNull();
    expect(result!.summary.total_requests).toBe(1247);
    expect(result!.by_model).toHaveLength(1);
    expect(result!.top_wallets).toHaveLength(1);
  });

  it("calls /v1/admin/stats with days param", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    const fetchSpy = mockFetch(200, statsPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchAdminStats(7);

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/v1/admin/stats?days=7");
  });

  it("includes Authorization header when GATEWAY_ADMIN_KEY is set", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    const fetchSpy = mockFetch(200, statsPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchAdminStats(30);

    const calledOptions = fetchSpy.mock.calls[0][1];
    expect(calledOptions.headers.Authorization).toBe(
      "Bearer test-secret-key",
    );
  });

  it("returns null without fetching when GATEWAY_ADMIN_KEY is not set", async () => {
    const fetchSpy = mockFetch(200, statsPayload);
    vi.stubGlobal("fetch", fetchSpy);

    const result = await fetchAdminStats(30);

    expect(result).toBeNull();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("returns null on non-ok response", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    vi.stubGlobal("fetch", mockFetch(403, {}));

    const result = await fetchAdminStats(30);

    expect(result).toBeNull();
  });

  it("returns null on network error", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new Error("Network error")),
    );

    const result = await fetchAdminStats(30);

    expect(result).toBeNull();
  });

  it("defaults to 30 days", async () => {
    process.env.GATEWAY_ADMIN_KEY = "test-secret-key";
    const fetchSpy = mockFetch(200, statsPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchAdminStats();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("days=30");
  });
});

// ─── fetchServices ────────────────────────────────────────────────────────────

describe("fetchServices", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const servicesPayload = {
    object: "list",
    data: [
      {
        id: "svc-1",
        name: "Test Service",
        description: "A test service",
        endpoint: "/v1/test",
      },
    ],
    total: 1,
  };

  it("returns parsed services on success", async () => {
    vi.stubGlobal("fetch", mockFetch(200, servicesPayload));

    const result = await fetchServices();

    expect(result).not.toBeNull();
    expect(result!.data).toHaveLength(1);
    expect(result!.total).toBe(1);
  });

  it("calls /v1/services endpoint", async () => {
    const fetchSpy = mockFetch(200, servicesPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchServices();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/v1/services");
  });

  it("returns null on non-ok response", async () => {
    vi.stubGlobal("fetch", mockFetch(500, {}));

    const result = await fetchServices();

    expect(result).toBeNull();
  });

  it("returns null on network error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new Error("Network error")),
    );

    const result = await fetchServices();

    expect(result).toBeNull();
  });
});

// ─── fetchEscrowConfig ────────────────────────────────────────────────────────

describe("fetchEscrowConfig", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const configPayload = {
    escrow_program_id: "EscrowXYZ123",
    current_slot: 250000000,
    network: "mainnet-beta",
    usdc_mint: "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    provider_wallet: "7xKpFqR9mRt",
  };

  it("returns parsed config on success", async () => {
    vi.stubGlobal("fetch", mockFetch(200, configPayload));

    const result = await fetchEscrowConfig();

    expect(result).not.toBeNull();
    expect(result!.network).toBe("mainnet-beta");
    expect(result!.escrow_program_id).toBe("EscrowXYZ123");
  });

  it("calls /v1/escrow/config endpoint", async () => {
    const fetchSpy = mockFetch(200, configPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchEscrowConfig();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/v1/escrow/config");
  });

  it("returns null on non-ok response", async () => {
    vi.stubGlobal("fetch", mockFetch(404, {}));

    const result = await fetchEscrowConfig();

    expect(result).toBeNull();
  });

  it("returns null on network error", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new Error("Network error")),
    );

    const result = await fetchEscrowConfig();

    expect(result).toBeNull();
  });
});

// ─── fetchEscrowHealth ────────────────────────────────────────────────────────

describe("fetchEscrowHealth", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    delete process.env.GATEWAY_ADMIN_KEY;
  });

  const healthPayload = {
    status: "ok",
    escrow_enabled: true,
    claim_processor_running: true,
    fee_payer_wallets: [],
    claims: { pending: 0, completed: 42 },
  };

  it("returns parsed health on success", async () => {
    process.env.GATEWAY_ADMIN_KEY = "admin-secret";
    vi.stubGlobal("fetch", mockFetch(200, healthPayload));

    const result = await fetchEscrowHealth();

    expect(result).not.toBeNull();
    expect(result!.escrow_enabled).toBe(true);
  });

  it("calls /v1/escrow/health endpoint", async () => {
    process.env.GATEWAY_ADMIN_KEY = "admin-secret";
    const fetchSpy = mockFetch(200, healthPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchEscrowHealth();

    const calledUrl: string = fetchSpy.mock.calls[0][0];
    expect(calledUrl).toContain("/v1/escrow/health");
  });

  it("includes Authorization header when GATEWAY_ADMIN_KEY is set", async () => {
    process.env.GATEWAY_ADMIN_KEY = "admin-secret";
    const fetchSpy = mockFetch(200, healthPayload);
    vi.stubGlobal("fetch", fetchSpy);

    await fetchEscrowHealth();

    const calledOptions = fetchSpy.mock.calls[0][1];
    expect(calledOptions.headers.Authorization).toBe("Bearer admin-secret");
  });

  it("returns null without fetching when GATEWAY_ADMIN_KEY is not set", async () => {
    const fetchSpy = mockFetch(200, healthPayload);
    vi.stubGlobal("fetch", fetchSpy);

    const result = await fetchEscrowHealth();

    expect(result).toBeNull();
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("returns null on non-ok response", async () => {
    process.env.GATEWAY_ADMIN_KEY = "admin-secret";
    vi.stubGlobal("fetch", mockFetch(401, {}));

    const result = await fetchEscrowHealth();

    expect(result).toBeNull();
  });

  it("returns null on network error", async () => {
    process.env.GATEWAY_ADMIN_KEY = "admin-secret";
    vi.stubGlobal(
      "fetch",
      vi.fn().mockRejectedValue(new Error("Network error")),
    );

    const result = await fetchEscrowHealth();

    expect(result).toBeNull();
  });
});
