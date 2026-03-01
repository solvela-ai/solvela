import { describe, it, expect } from "vitest";
import {
  formatUSDC,
  formatNumber,
  providerColor,
  providerBadgeClass,
  shortAddress,
  cn,
} from "@/lib/utils";

// ─── formatUSDC ───────────────────────────────────────────────────────────────

describe("formatUSDC", () => {
  it("returns $0.00 for zero", () => {
    expect(formatUSDC(0)).toBe("$0.00");
  });

  it("uses exponential notation for very small amounts", () => {
    expect(formatUSDC(0.00001)).toBe("$1.00e-5");
  });

  it("formats normal amounts with 4 decimal places by default", () => {
    expect(formatUSDC(1.5)).toBe("$1.5000");
  });

  it("respects custom decimals parameter", () => {
    expect(formatUSDC(1.5, 2)).toBe("$1.50");
  });

  it("formats the boundary value 0.0001 with fixed notation", () => {
    expect(formatUSDC(0.0001)).toBe("$0.0001");
  });

  it("formats amounts just below 0.0001 with exponential notation", () => {
    const result = formatUSDC(0.00009);
    expect(result.startsWith("$")).toBe(true);
    expect(result).toContain("e");
  });

  it("handles large amounts", () => {
    expect(formatUSDC(1234.5678)).toBe("$1234.5678");
  });
});

// ─── formatNumber ─────────────────────────────────────────────────────────────

describe("formatNumber", () => {
  it("returns plain string for numbers under 1000", () => {
    expect(formatNumber(42)).toBe("42");
    expect(formatNumber(999)).toBe("999");
  });

  it("formats thousands with k suffix", () => {
    expect(formatNumber(1000)).toBe("1.0k");
    expect(formatNumber(1500)).toBe("1.5k");
    expect(formatNumber(999_999)).toBe("1000.0k");
  });

  it("formats millions with M suffix", () => {
    expect(formatNumber(1_000_000)).toBe("1.0M");
    expect(formatNumber(2_500_000)).toBe("2.5M");
  });

  it("handles zero", () => {
    expect(formatNumber(0)).toBe("0");
  });
});

// ─── providerColor ────────────────────────────────────────────────────────────

describe("providerColor", () => {
  it("returns correct color for openai", () => {
    expect(providerColor("openai")).toBe("#10a37f");
  });

  it("returns correct color for anthropic", () => {
    expect(providerColor("anthropic")).toBe("#d97757");
  });

  it("returns correct color for google", () => {
    expect(providerColor("google")).toBe("#4285f4");
  });

  it("returns correct color for xai", () => {
    expect(providerColor("xai")).toBe("#1a1a1a");
  });

  it("returns correct color for deepseek", () => {
    expect(providerColor("deepseek")).toBe("#536dfe");
  });

  it("is case-insensitive", () => {
    expect(providerColor("OpenAI")).toBe("#10a37f");
    expect(providerColor("ANTHROPIC")).toBe("#d97757");
  });

  it("returns fallback gray for unknown provider", () => {
    expect(providerColor("unknown")).toBe("#6b7280");
    expect(providerColor("cohere")).toBe("#6b7280");
  });
});

// ─── providerBadgeClass ───────────────────────────────────────────────────────

describe("providerBadgeClass", () => {
  it("returns emerald classes for openai", () => {
    expect(providerBadgeClass("openai")).toBe("bg-emerald-100 text-emerald-800");
  });

  it("returns orange classes for anthropic", () => {
    expect(providerBadgeClass("anthropic")).toBe("bg-orange-100 text-orange-800");
  });

  it("returns blue classes for google", () => {
    expect(providerBadgeClass("google")).toBe("bg-blue-100 text-blue-800");
  });

  it("returns gray classes for xai", () => {
    expect(providerBadgeClass("xai")).toBe("bg-gray-100 text-gray-800");
  });

  it("returns indigo classes for deepseek", () => {
    expect(providerBadgeClass("deepseek")).toBe("bg-indigo-100 text-indigo-800");
  });

  it("is case-insensitive", () => {
    expect(providerBadgeClass("OpenAI")).toBe("bg-emerald-100 text-emerald-800");
  });

  it("returns fallback classes for unknown provider", () => {
    expect(providerBadgeClass("mistral")).toBe("bg-gray-100 text-gray-700");
  });
});

// ─── shortAddress ─────────────────────────────────────────────────────────────

describe("shortAddress", () => {
  it("returns address unchanged if 10 chars or fewer", () => {
    expect(shortAddress("1234567890")).toBe("1234567890");
    expect(shortAddress("short")).toBe("short");
  });

  it("truncates long addresses with ellipsis", () => {
    const addr = "4xKp9mRt2nSv8kJx";
    expect(shortAddress(addr)).toBe("4xKp...8kJx");
  });

  it("preserves first 4 and last 4 characters", () => {
    const addr = "ABCD_MIDDLE_SECTION_EFGH";
    const result = shortAddress(addr);
    expect(result.startsWith("ABCD")).toBe(true);
    expect(result.endsWith("EFGH")).toBe(true);
    expect(result).toContain("...");
  });

  it("handles exactly 11 characters", () => {
    const addr = "12345678901";
    expect(shortAddress(addr)).toBe("1234...8901");
  });
});

// ─── cn ───────────────────────────────────────────────────────────────────────

describe("cn", () => {
  it("merges class names", () => {
    const result = cn("foo", "bar");
    expect(result).toContain("foo");
    expect(result).toContain("bar");
  });

  it("handles conditional classes", () => {
    const result = cn("base", false && "hidden", "visible");
    expect(result).toContain("base");
    expect(result).toContain("visible");
    expect(result).not.toContain("hidden");
  });

  it("handles empty input", () => {
    expect(cn()).toBe("");
  });
});
