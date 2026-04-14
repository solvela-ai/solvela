import { AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { ModelPie } from "@/components/charts/model-pie";
import { SpendChart } from "@/components/charts/spend-chart";
import { Badge } from "@/components/ui/badge";
import { MODEL_USAGE, SPEND_HISTORY } from "@/lib/mock-data";
import { fetchAdminStats } from "@/lib/api";
import {
  formatUSDC,
  formatNumber,
  providerBadgeClass,
} from "@/lib/utils";
import type { SpendDataPoint, ModelUsage } from "@/types";

export default async function UsagePage() {
  const statsResponse = await fetchAdminStats(30);

  const usingMockData = !statsResponse;

  // Map by_model to ModelUsage[], falling back to mock data
  const modelUsage: ModelUsage[] = statsResponse
    ? (() => {
        const totalRequests = statsResponse.by_model.reduce(
          (sum, m) => sum + m.requests,
          0,
        );
        return statsResponse.by_model.map((m) => ({
          model: m.model,
          provider: m.provider,
          requests: m.requests,
          spend: parseFloat(m.cost_usdc),
          pct:
            totalRequests > 0
              ? Math.round((m.requests / totalRequests) * 100)
              : 0,
        }));
      })()
    : MODEL_USAGE;

  // Map by_day to SpendDataPoint[], falling back to mock data
  const history: SpendDataPoint[] = statsResponse
    ? statsResponse.by_day.map((day) => ({
        date: new Date(day.date).toLocaleDateString("en-US", {
          month: "short",
          day: "numeric",
        }),
        spend: day.spend,
        requests: day.requests,
      }))
    : SPEND_HISTORY;

  // Top wallets from API or hardcoded fallback
  const topWallets = statsResponse
    ? statsResponse.top_wallets.map((w) => {
        const totalCost = parseFloat(statsResponse.summary.total_cost_usdc);
        const walletCost = parseFloat(w.cost_usdc);
        return {
          wallet: w.wallet,
          spend: walletCost,
          requests: w.requests,
          pct: totalCost > 0 ? Math.round((walletCost / totalCost) * 100) : 0,
        };
      })
    : [
        { wallet: "7xKpF...mR9t", spend: 0.62, requests: 412, pct: 55 },
        { wallet: "3yLqZ...nS2v", spend: 0.31, requests: 278, pct: 27 },
        { wallet: "9bNwA...8kJx", spend: 0.20, requests: 237, pct: 18 },
      ];

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Usage"
        subtitle="Per-model and per-wallet breakdown"
      />

      <div className="flex-1 p-6 space-y-6">
        {/* Mock data warning banner */}
        {usingMockData && (
          <div className="flex items-center gap-2 rounded-lg border border-warning/20 bg-warning/10 px-4 py-2.5 text-sm text-warning">
            <AlertTriangle size={14} className="flex-shrink-0" />
            <span>
              Unable to reach gateway API. Showing sample data.
            </span>
          </div>
        )}

        {/* Top row: pie + spend line */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <div className="rounded-xl border border-border bg-bg-surface p-5">
            <h2 className="text-sm font-semibold text-text-primary mb-4">
              Requests by Model
            </h2>
            <ModelPie data={modelUsage} />
          </div>

          <div className="rounded-xl border border-border bg-bg-surface p-5">
            <h2 className="text-sm font-semibold text-text-primary mb-4">
              Spend Over Time
            </h2>
            <SpendChart data={history} />
          </div>
        </div>

        {/* Model breakdown table */}
        <div className="rounded-xl border border-border bg-bg-surface overflow-hidden">
          <div className="px-5 py-4 border-b border-border-subtle">
            <h2 className="text-sm font-semibold text-text-primary">
              Model Breakdown
            </h2>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-bg-surface-hover text-xs text-text-secondary uppercase tracking-wide">
                  <th className="px-5 py-3 text-left font-medium">Model</th>
                  <th className="px-5 py-3 text-left font-medium">Provider</th>
                  <th className="px-5 py-3 text-right font-medium">Requests</th>
                  <th className="px-5 py-3 text-right font-medium">Spend (USDC)</th>
                  <th className="px-5 py-3 text-right font-medium">Share</th>
                  <th className="px-5 py-3 text-left font-medium">Distribution</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border-subtle">
                {modelUsage.map((row) => (
                  <tr key={row.model} className="hover:bg-bg-surface-hover transition-colors">
                    <td className="px-5 py-3 font-medium text-text-primary">
                      {row.model}
                    </td>
                    <td className="px-5 py-3">
                      <Badge className={providerBadgeClass(row.provider)}>
                        {row.provider}
                      </Badge>
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary">
                      {formatNumber(row.requests)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary">
                      {formatUSDC(row.spend, 4)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary">
                      {row.pct}%
                    </td>
                    <td className="px-5 py-3">
                      <div className="h-1.5 w-32 rounded-full bg-bg-surface-hover">
                        <div
                          className="h-1.5 rounded-full bg-brand"
                          style={{ width: `${row.pct}%` }}
                        />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        {/* Wallet breakdown */}
        <div className="rounded-xl border border-border bg-bg-surface p-5">
          <h2 className="text-sm font-semibold text-text-primary mb-4">
            Top Wallets by Spend
          </h2>
          <div className="space-y-3">
            {topWallets.map((w) => (
              <div key={w.wallet} className="flex items-center gap-4">
                <code className="w-28 text-xs text-text-secondary font-mono">
                  {w.wallet}
                </code>
                <div className="flex-1 h-1.5 rounded-full bg-bg-surface-hover">
                  <div
                    className="h-1.5 rounded-full bg-info"
                    style={{ width: `${w.pct}%` }}
                  />
                </div>
                <span className="w-20 text-right text-xs tabular-nums text-text-secondary">
                  {formatUSDC(w.spend, 2)}
                </span>
                <span className="w-16 text-right text-xs text-text-tertiary">
                  {formatNumber(w.requests)} req
                </span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}
