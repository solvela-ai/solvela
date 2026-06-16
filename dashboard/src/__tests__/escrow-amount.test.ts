import { describe, it, expect } from "vitest";

import {
  usdcDecimalToAtomic,
  atomicToUsdcDisplay,
} from "@/lib/escrow/amount";

// USDC is 6-decimal atomic micro-USDC; all math is integer / BigInt, fail-closed.
// The gateway rejects non-canonical amount strings, so usdcDecimalToAtomic must
// emit a canonical integer string (no leading zeros).

describe("usdcDecimalToAtomic", () => {
  it("converts a whole number to atomic micro-USDC", () => {
    expect(usdcDecimalToAtomic("1")).toBe("1000000");
  });

  it("converts the canonical golden-vector fraction 0.002625 to 2625", () => {
    expect(usdcDecimalToAtomic("0.002625")).toBe("2625");
  });

  it("converts a half dollar to 500000", () => {
    expect(usdcDecimalToAtomic("0.5")).toBe("500000");
  });

  it("converts the smallest unit 0.000001 to 1", () => {
    expect(usdcDecimalToAtomic("0.000001")).toBe("1");
  });

  it("pads a short fraction to exactly 6 digits", () => {
    expect(usdcDecimalToAtomic("1.5")).toBe("1500000");
    expect(usdcDecimalToAtomic("12.34")).toBe("12340000");
  });

  it("accepts a value at exactly 6 fractional digits", () => {
    expect(usdcDecimalToAtomic("1.234567")).toBe("1234567");
  });

  it("accepts a trailing-dot integer", () => {
    expect(usdcDecimalToAtomic("7.")).toBe("7000000");
  });

  it("accepts a leading-dot fraction", () => {
    expect(usdcDecimalToAtomic(".5")).toBe("500000");
  });

  it("round-trips through atomicToUsdcDisplay", () => {
    for (const dec of ["1", "0.002625", "0.5", "0.000001", "1234.567890"]) {
      const atomic = usdcDecimalToAtomic(dec);
      const back = atomicToUsdcDisplay(atomic);
      // Re-canonicalize the input decimal the same way display would format it.
      expect(usdcDecimalToAtomic(back)).toBe(atomic);
    }
  });

  it("accepts a large value just below the u64 ceiling", () => {
    // u64 max = 18446744073709551615 atomic = 18446744073709.551615 USDC.
    expect(usdcDecimalToAtomic("18446744073709.551615")).toBe(
      "18446744073709551615",
    );
  });

  // ── fail-closed rejections ────────────────────────────────────────────────

  it("rejects empty string", () => {
    expect(() => usdcDecimalToAtomic("")).toThrow();
  });

  it("rejects a negative amount", () => {
    expect(() => usdcDecimalToAtomic("-1")).toThrow();
    expect(() => usdcDecimalToAtomic("-0.5")).toThrow();
  });

  it("rejects an explicit + sign", () => {
    expect(() => usdcDecimalToAtomic("+1")).toThrow();
  });

  it("rejects surrounding whitespace", () => {
    expect(() => usdcDecimalToAtomic(" 1")).toThrow();
    expect(() => usdcDecimalToAtomic("1 ")).toThrow();
  });

  it("rejects more than 6 fractional digits", () => {
    expect(() => usdcDecimalToAtomic("1.2345678")).toThrow();
    expect(() => usdcDecimalToAtomic("0.0000001")).toThrow();
  });

  it("rejects non-numeric input", () => {
    expect(() => usdcDecimalToAtomic("abc")).toThrow();
    expect(() => usdcDecimalToAtomic("1.2.3")).toThrow();
    expect(() => usdcDecimalToAtomic("1e6")).toThrow();
    expect(() => usdcDecimalToAtomic("0x10")).toThrow();
    expect(() => usdcDecimalToAtomic("NaN")).toThrow();
    expect(() => usdcDecimalToAtomic("Infinity")).toThrow();
  });

  it("rejects zero (gateway requires amount > 0)", () => {
    expect(() => usdcDecimalToAtomic("0")).toThrow();
    expect(() => usdcDecimalToAtomic("0.0")).toThrow();
    expect(() => usdcDecimalToAtomic("0.000000")).toThrow();
    expect(() => usdcDecimalToAtomic(".0")).toThrow();
  });

  it("rejects a value that overflows u64", () => {
    // u64 max + 1 atomic.
    expect(() => usdcDecimalToAtomic("18446744073709.551616")).toThrow();
    expect(() => usdcDecimalToAtomic("99999999999999")).toThrow();
  });

  it("rejects a bare dot", () => {
    expect(() => usdcDecimalToAtomic(".")).toThrow();
  });
});

describe("atomicToUsdcDisplay", () => {
  it("formats the golden-vector amount", () => {
    expect(atomicToUsdcDisplay("2625")).toBe("0.002625");
  });

  it("formats a whole-dollar amount", () => {
    expect(atomicToUsdcDisplay("1000000")).toBe("1");
  });

  it("formats the smallest unit", () => {
    expect(atomicToUsdcDisplay("1")).toBe("0.000001");
    expect(atomicToUsdcDisplay(BigInt(1))).toBe("0.000001");
  });

  it("trims trailing fractional zeros but keeps significant ones", () => {
    expect(atomicToUsdcDisplay("1500000")).toBe("1.5");
    expect(atomicToUsdcDisplay("1234567")).toBe("1.234567");
    expect(atomicToUsdcDisplay("12340000")).toBe("12.34");
  });

  it("accepts a bigint input", () => {
    expect(atomicToUsdcDisplay(BigInt(2625))).toBe("0.002625");
  });

  it("formats zero", () => {
    expect(atomicToUsdcDisplay("0")).toBe("0");
    expect(atomicToUsdcDisplay(BigInt(0))).toBe("0");
  });

  it("rejects a malformed atomic string", () => {
    expect(() => atomicToUsdcDisplay("abc")).toThrow();
    expect(() => atomicToUsdcDisplay("-1")).toThrow();
    expect(() => atomicToUsdcDisplay("1.5")).toThrow();
  });
});
