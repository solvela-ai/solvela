import type { DashboardStats, ModelUsage, SpendDataPoint, WalletTx } from "@/types";

// ─── Static spend history (last 30 days) ─────────────────────────────────────
// Using static fixtures to avoid Math.random() hydration mismatches.
// In production, replace with real data from the usage tracker API.

export const SPEND_HISTORY: SpendDataPoint[] = [
  { date: "Mar 16", spend: 3.12,  requests: 312 },
  { date: "Mar 17", spend: 5.84,  requests: 481 },
  { date: "Mar 18", spend: 7.43,  requests: 612 },
  { date: "Mar 19", spend: 4.91,  requests: 404 },
  { date: "Mar 20", spend: 9.27,  requests: 763 },
  { date: "Mar 21", spend: 6.38,  requests: 525 },
  { date: "Mar 22", spend: 4.05,  requests: 333 },
  { date: "Mar 23", spend: 8.61,  requests: 709 },
  { date: "Mar 24", spend: 11.24, requests: 924 },
  { date: "Mar 25", spend: 6.74,  requests: 554 },
  { date: "Mar 26", spend: 5.19,  requests: 427 },
  { date: "Mar 27", spend: 8.82,  requests: 726 },
  { date: "Mar 28", spend: 10.53, requests: 866 },
  { date: "Mar 29", spend: 7.88,  requests: 649 },
  { date: "Mar 30", spend: 4.24,  requests: 349 },
  { date: "Mar 31", spend: 9.64,  requests: 793 },
  { date: "Apr 1",  spend: 11.89, requests: 978 },
  { date: "Apr 2",  spend: 6.38,  requests: 525 },
  { date: "Apr 3",  spend: 8.12,  requests: 668 },
  { date: "Apr 4",  spend: 10.53, requests: 867 },
  { date: "Apr 5",  spend: 7.44,  requests: 613 },
  { date: "Apr 6",  spend: 5.02,  requests: 413 },
  { date: "Apr 7",  spend: 9.17,  requests: 754 },
  { date: "Apr 8",  spend: 11.31, requests: 931 },
  { date: "Apr 9",  spend: 6.84,  requests: 563 },
  { date: "Apr 10", spend: 8.47,  requests: 697 },
  { date: "Apr 11", spend: 12.03, requests: 990 },
  { date: "Apr 12", spend: 5.76,  requests: 474 },
  { date: "Apr 13", spend: 9.41,  requests: 774 },
  { date: "Apr 14", spend: 10.17, requests: 836 },
];

// ─── Mock model usage breakdown ───────────────────────────────────────────────

export const MODEL_USAGE: ModelUsage[] = [
  { model: "claude-sonnet-4-5",   provider: "anthropic", requests: 3842, spend: 97.84,  pct: 31 },
  { model: "gpt-4o-mini",         provider: "openai",    requests: 3104, spend: 22.37,  pct: 25 },
  { model: "gemini-2.5-flash",    provider: "google",    requests: 2187, spend: 11.42,  pct: 18 },
  { model: "gpt-4o",              provider: "openai",    requests:  991, spend: 58.63,  pct:  8 },
  { model: "claude-opus-4",       provider: "anthropic", requests:  744, spend: 41.29,  pct:  6 },
  { model: "deepseek-v3",         provider: "deepseek",  requests:  618, spend:  4.91,  pct:  5 },
  { model: "gemini-2.5-pro",      provider: "google",    requests:  432, spend:  7.83,  pct:  3 },
  { model: "grok-3-mini",         provider: "xai",       requests:  482, spend:  3.54,  pct:  4 },
];

// ─── Mock wallet transactions ─────────────────────────────────────────────────

export const WALLET_TXS: WalletTx[] = [
  {
    signature: "4xKp7r...9mRtQv",
    model: "claude-sonnet-4-5",
    amount: "0.002625",
    timestamp: "2 min ago",
    status: "confirmed",
  },
  {
    signature: "7yLqNs...2nSvBk",
    model: "gpt-4o-mini",
    amount: "0.000094",
    timestamp: "18 min ago",
    status: "confirmed",
  },
  {
    signature: "2bNwHp...8kJxMc",
    model: "gemini-2.5-flash",
    amount: "0.000210",
    timestamp: "1 hr ago",
    status: "confirmed",
  },
  {
    signature: "9cRmZd...5pTyWe",
    model: "gpt-4o",
    amount: "0.004200",
    timestamp: "3 hr ago",
    status: "confirmed",
  },
  {
    signature: "1dVzAf...3qUwLj",
    model: "deepseek-v3",
    amount: "0.000084",
    timestamp: "5 hr ago",
    status: "confirmed",
  },
  {
    signature: "6eWxBg...7rVsKm",
    model: "claude-opus-4",
    amount: "0.018340",
    timestamp: "7 hr ago",
    status: "confirmed",
  },
];

// ─── Mock dashboard stats ─────────────────────────────────────────────────────

export const DASHBOARD_STATS: DashboardStats = {
  totalSpend: 247.83,
  totalRequests: 12400,
  avgCostPerRequest: 0.01999,
  savingsVsOpenAI: 47,
  activeModels: 8,
  walletBalance: 312.50,
};
