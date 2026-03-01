import { describe, it, expect } from "vitest";
import {
  SPEND_HISTORY,
  MODEL_USAGE,
  WALLET_TXS,
  DASHBOARD_STATS,
} from "@/lib/mock-data";

// ─── SPEND_HISTORY ────────────────────────────────────────────────────────────

describe("SPEND_HISTORY", () => {
  it("has exactly 30 data points", () => {
    expect(SPEND_HISTORY).toHaveLength(30);
  });

  it("each entry has date, spend, and requests fields", () => {
    for (const point of SPEND_HISTORY) {
      expect(typeof point.date).toBe("string");
      expect(typeof point.spend).toBe("number");
      expect(typeof point.requests).toBe("number");
    }
  });

  it("spend values are positive", () => {
    for (const point of SPEND_HISTORY) {
      expect(point.spend).toBeGreaterThan(0);
    }
  });

  it("request counts are positive integers", () => {
    for (const point of SPEND_HISTORY) {
      expect(point.requests).toBeGreaterThan(0);
      expect(Number.isInteger(point.requests)).toBe(true);
    }
  });

  it("is deterministic — not generated with Math.random()", () => {
    // Same reference two calls in a row must be identical
    expect(SPEND_HISTORY[0].spend).toBe(SPEND_HISTORY[0].spend);
    expect(SPEND_HISTORY[0].requests).toBe(SPEND_HISTORY[0].requests);
  });

  it("first entry is Feb 1", () => {
    expect(SPEND_HISTORY[0].date).toBe("Feb 1");
  });

  it("last entry is Mar 2", () => {
    expect(SPEND_HISTORY[29].date).toBe("Mar 2");
  });
});

// ─── MODEL_USAGE ──────────────────────────────────────────────────────────────

describe("MODEL_USAGE", () => {
  it("has at least one entry", () => {
    expect(MODEL_USAGE.length).toBeGreaterThan(0);
  });

  it("each entry has required fields", () => {
    for (const entry of MODEL_USAGE) {
      expect(typeof entry.model).toBe("string");
      expect(typeof entry.provider).toBe("string");
      expect(typeof entry.requests).toBe("number");
      expect(typeof entry.spend).toBe("number");
      expect(typeof entry.pct).toBe("number");
    }
  });

  it("pct values sum to approximately 100", () => {
    const total = MODEL_USAGE.reduce((acc, e) => acc + e.pct, 0);
    expect(total).toBe(100);
  });

  it("each pct value is between 0 and 100", () => {
    for (const entry of MODEL_USAGE) {
      expect(entry.pct).toBeGreaterThan(0);
      expect(entry.pct).toBeLessThanOrEqual(100);
    }
  });

  it("has 5 model entries", () => {
    expect(MODEL_USAGE).toHaveLength(5);
  });
});

// ─── WALLET_TXS ───────────────────────────────────────────────────────────────

describe("WALLET_TXS", () => {
  it("has at least one transaction", () => {
    expect(WALLET_TXS.length).toBeGreaterThan(0);
  });

  it("each tx has required fields", () => {
    for (const tx of WALLET_TXS) {
      expect(typeof tx.signature).toBe("string");
      expect(typeof tx.model).toBe("string");
      expect(typeof tx.amount).toBe("string");
      expect(typeof tx.timestamp).toBe("string");
      expect(["confirmed", "pending", "failed"]).toContain(tx.status);
    }
  });

  it("amount fields are numeric strings", () => {
    for (const tx of WALLET_TXS) {
      expect(isNaN(Number(tx.amount))).toBe(false);
    }
  });
});

// ─── DASHBOARD_STATS ─────────────────────────────────────────────────────────

describe("DASHBOARD_STATS", () => {
  it("has all required stat fields", () => {
    expect(typeof DASHBOARD_STATS.totalSpend).toBe("number");
    expect(typeof DASHBOARD_STATS.totalRequests).toBe("number");
    expect(typeof DASHBOARD_STATS.avgCostPerRequest).toBe("number");
    expect(typeof DASHBOARD_STATS.savingsVsOpenAI).toBe("number");
    expect(typeof DASHBOARD_STATS.activeModels).toBe("number");
    expect(typeof DASHBOARD_STATS.walletBalance).toBe("number");
  });

  it("totalRequests is a positive integer", () => {
    expect(DASHBOARD_STATS.totalRequests).toBeGreaterThan(0);
    expect(Number.isInteger(DASHBOARD_STATS.totalRequests)).toBe(true);
  });

  it("savingsVsOpenAI is a percentage between 0 and 100", () => {
    expect(DASHBOARD_STATS.savingsVsOpenAI).toBeGreaterThanOrEqual(0);
    expect(DASHBOARD_STATS.savingsVsOpenAI).toBeLessThanOrEqual(100);
  });

  it("avgCostPerRequest is consistent with totalSpend and totalRequests", () => {
    const computed = DASHBOARD_STATS.totalSpend / DASHBOARD_STATS.totalRequests;
    expect(Math.abs(computed - DASHBOARD_STATS.avgCostPerRequest)).toBeLessThan(0.001);
  });
});
