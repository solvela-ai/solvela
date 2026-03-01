import type { DashboardStats, ModelUsage, SpendDataPoint, WalletTx } from "@/types";

// ─── Mock spend history (last 30 days) ───────────────────────────────────────

export function generateSpendHistory(): SpendDataPoint[] {
  const data: SpendDataPoint[] = [];
  const now = new Date();
  for (let i = 29; i >= 0; i--) {
    const d = new Date(now);
    d.setDate(d.getDate() - i);
    const base = 0.04 + Math.random() * 0.12;
    data.push({
      date: d.toLocaleDateString("en-US", { month: "short", day: "numeric" }),
      spend: parseFloat(base.toFixed(4)),
      requests: Math.floor(10 + Math.random() * 40),
    });
  }
  return data;
}

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
