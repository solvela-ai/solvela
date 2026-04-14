import { clsx, type ClassValue } from 'clsx'
import { twMerge } from 'tailwind-merge'

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
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
    openai: "bg-emerald-500/15 text-emerald-400",
    anthropic: "bg-orange-500/15 text-orange-400",
    google: "bg-blue-500/15 text-blue-400",
    xai: "bg-gray-500/15 text-gray-400",
    deepseek: "bg-indigo-500/15 text-indigo-400",
  };
  return map[provider.toLowerCase()] ?? "bg-gray-500/15 text-gray-400";
}

export function shortAddress(addr: string): string {
  if (addr.length <= 10) return addr;
  return `${addr.slice(0, 4)}...${addr.slice(-4)}`;
}
