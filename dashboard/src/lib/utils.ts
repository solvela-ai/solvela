import { clsx, type ClassValue } from "clsx";

export function cn(...inputs: ClassValue[]) {
  return clsx(inputs);
}

export function formatUSDC(amount: number, decimals = 4): string {
  if (amount === 0) return "$0.00";
  if (amount < 0.0001) return `$${amount.toExponential(2)}`;
  return `$${amount.toFixed(decimals)}`;
}

export function formatNumber(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return n.toString();
}

export function providerColor(provider: string): string {
  const map: Record<string, string> = {
    openai: "#10a37f",
    anthropic: "#d97757",
    google: "#4285f4",
    xai: "#1a1a1a",
    deepseek: "#536dfe",
  };
  return map[provider.toLowerCase()] ?? "#6b7280";
}

export function providerBadgeClass(provider: string): string {
  const map: Record<string, string> = {
    openai: "bg-emerald-100 text-emerald-800",
    anthropic: "bg-orange-100 text-orange-800",
    google: "bg-blue-100 text-blue-800",
    xai: "bg-gray-100 text-gray-800",
    deepseek: "bg-indigo-100 text-indigo-800",
  };
  return map[provider.toLowerCase()] ?? "bg-gray-100 text-gray-700";
}

export function shortAddress(addr: string): string {
  if (addr.length <= 10) return addr;
  return `${addr.slice(0, 4)}...${addr.slice(-4)}`;
}
