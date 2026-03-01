import type { DashboardStats, ModelUsage, SpendDataPoint, WalletTx } from "@/types";

// ─── Static spend history (last 30 days) ─────────────────────────────────────
// Using static fixtures to avoid Math.random() hydration mismatches.
// In production, replace with real data from the usage tracker API.

export const SPEND_HISTORY: SpendDataPoint[] = [
  { date: "Feb 1",  spend: 0.0412, requests: 14 },
  { date: "Feb 2",  spend: 0.0618, requests: 22 },
  { date: "Feb 3",  spend: 0.0891, requests: 31 },
  { date: "Feb 4",  spend: 0.0524, requests: 18 },
  { date: "Feb 5",  spend: 0.1103, requests: 38 },
  { date: "Feb 6",  spend: 0.0732, requests: 25 },
  { date: "Feb 7",  spend: 0.0457, requests: 16 },
  { date: "Feb 8",  spend: 0.0985, requests: 34 },
  { date: "Feb 9",  spend: 0.1241, requests: 43 },
  { date: "Feb 10", spend: 0.0663, requests: 23 },
  { date: "Feb 11", spend: 0.0518, requests: 18 },
  { date: "Feb 12", spend: 0.0874, requests: 30 },
  { date: "Feb 13", spend: 0.1052, requests: 37 },
  { date: "Feb 14", spend: 0.0791, requests: 28 },
  { date: "Feb 15", spend: 0.0423, requests: 15 },
  { date: "Feb 16", spend: 0.0967, requests: 34 },
  { date: "Feb 17", spend: 0.1184, requests: 41 },
  { date: "Feb 18", spend: 0.0639, requests: 22 },
  { date: "Feb 19", spend: 0.0812, requests: 29 },
  { date: "Feb 20", spend: 0.1056, requests: 37 },
  { date: "Feb 21", spend: 0.0744, requests: 26 },
  { date: "Feb 22", spend: 0.0503, requests: 18 },
  { date: "Feb 23", spend: 0.0921, requests: 32 },
  { date: "Feb 24", spend: 0.1137, requests: 40 },
  { date: "Feb 25", spend: 0.0685, requests: 24 },
  { date: "Feb 26", spend: 0.0847, requests: 30 },
  { date: "Feb 27", spend: 0.1203, requests: 42 },
  { date: "Feb 28", spend: 0.0578, requests: 20 },
  { date: "Mar 1",  spend: 0.0942, requests: 33 },
  { date: "Mar 2",  spend: 0.1018, requests: 36 },
];

// ─── Mock model usage breakdown ───────────────────────────────────────────────

export const MODEL_USAGE: ModelUsage[] = [
  { model: "claude-sonnet-4", provider: "anthropic", requests: 312, spend: 0.48, pct: 38 },
  { model: "gpt-4o-mini",     provider: "openai",    requests: 278, spend: 0.21, pct: 26 },
  { model: "gemini-2.5-flash",provider: "google",    requests: 195, spend: 0.09, pct: 18 },
  { model: "gpt-4o",          provider: "openai",    requests:  88, spend: 0.31, pct: 11 },
  { model: "deepseek-v3",     provider: "deepseek",  requests:  54, spend: 0.04, pct:  7 },
];

// ─── Mock wallet transactions ─────────────────────────────────────────────────

export const WALLET_TXS: WalletTx[] = [
  {
    signature: "4xKp...9mRt",
    model: "claude-sonnet-4",
    amount: "0.002625",
    timestamp: "2 min ago",
    status: "confirmed",
  },
  {
    signature: "7yLq...2nSv",
    model: "gpt-4o-mini",
    amount: "0.000094",
    timestamp: "18 min ago",
    status: "confirmed",
  },
  {
    signature: "2bNw...8kJx",
    model: "gemini-2.5-flash",
    amount: "0.000210",
    timestamp: "1 hr ago",
    status: "confirmed",
  },
  {
    signature: "9cRm...5pTy",
    model: "gpt-4o",
    amount: "0.004200",
    timestamp: "3 hr ago",
    status: "confirmed",
  },
  {
    signature: "1dVz...3qUw",
    model: "deepseek-v3",
    amount: "0.000084",
    timestamp: "5 hr ago",
    status: "confirmed",
  },
];

// ─── Mock dashboard stats ─────────────────────────────────────────────────────

export const DASHBOARD_STATS: DashboardStats = {
  totalSpend: 1.13,
  totalRequests: 927,
  avgCostPerRequest: 0.00122,
  savingsVsOpenAI: 47,
  activeModels: 5,
  walletBalance: 12.44,
};
