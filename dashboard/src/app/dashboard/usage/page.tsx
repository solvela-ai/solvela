import { AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { TerminalCard } from "@/components/ui/terminal-card";
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
        { wallet: "7xKpF2mVnR...9tQz4BsLw", spend: 136.31, requests: 5104, pct: 55 },
        { wallet: "3yLqZnS8Bk...2vXj7HdEm", spend: 66.91,  requests: 3348, pct: 27 },
        { wallet: "9bNwHp4Jxc...8kMc1FrGa", spend: 44.61,  requests: 3948, pct: 18 },
      ];

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Usage"
        subtitle="Per-model and per-wallet breakdown"
      />

      <div className="flex-1 p-6 space-y-5">
        {/* Mock data warning */}
        {usingMockData && (
          <div role="status" aria-live="polite" className="flex items-center gap-2 rounded border border-border px-4 py-2.5 text-sm text-text-secondary">
            <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
            <span>Gateway offline — showing sample data.</span>
          </div>
        )}

        {/* Charts row */}
        <div>
          <p className="eyebrow mb-3">Distribution</p>
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
            <TerminalCard
              title="requests.by.model"
              meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
            >
              <ModelPie data={modelUsage} />
            </TerminalCard>

            <TerminalCard
              title="spend.over.time"
              meta={<span className="text-xxs text-text-tertiary font-mono">USDC · 30d</span>}
            >
              <SpendChart data={history} />
            </TerminalCard>
          </div>
        </div>

        {/* Model breakdown table */}
        <TerminalCard title="model.breakdown" bare className="overflow-hidden">
          <div className="overflow-x-auto bg-popover">
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border text-xs text-text-tertiary uppercase tracking-wide font-mono">
                  <th className="px-5 py-2.5 text-left font-medium">Model</th>
                  <th className="px-5 py-2.5 text-left font-medium">Provider</th>
                  <th className="px-5 py-2.5 text-right font-medium">Requests</th>
                  <th className="px-5 py-2.5 text-right font-medium">Spend</th>
                  <th className="px-5 py-2.5 text-right font-medium">Share</th>
                  <th className="px-5 py-2.5 text-left font-medium">Bar</th>
                </tr>
              </thead>
              <tbody>
                {modelUsage.map((row, i) => (
                  <tr
                    key={row.model}
                    className={`border-b border-border last:border-0 hover:bg-bg-surface transition-colors ${i === 0 ? "text-text-primary" : ""}`}
                  >
                    <td className="px-5 py-3 font-medium text-text-primary font-mono text-xs">
                      {row.model}
                    </td>
                    <td className="px-5 py-3">
                      <Badge className={providerBadgeClass(row.provider)}>
                        {row.provider}
                      </Badge>
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary text-xs font-mono">
                      {formatNumber(row.requests)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary text-xs font-mono">
                      {formatUSDC(row.spend, 2)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-text-secondary text-xs font-mono">
                      {row.pct}%
                    </td>
                    <td className="px-5 py-3">
                      <div className="h-1 w-32 bg-bg-surface-raised">
                        <div
                          className="h-1 bg-text-tertiary"
                          style={{ width: `${row.pct}%` }}
                        />
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </TerminalCard>

        {/* Wallet breakdown */}
        <TerminalCard title="wallets.by.spend">
          <div className="space-y-3">
            {topWallets.map((w) => (
              <div key={w.wallet} className="flex items-center gap-4">
                <code className="w-36 text-xs text-text-tertiary font-mono truncate">
                  {w.wallet}
                </code>
                <div className="flex-1 h-px bg-bg-surface-raised relative">
                  <div
                    className="absolute inset-y-0 left-0 bg-text-tertiary"
                    style={{ width: `${w.pct}%`, height: "1px" }}
                  />
                </div>
                <span className="w-20 text-right text-xs tabular-nums text-text-secondary font-mono">
                  {formatUSDC(w.spend, 2)}
                </span>
                <span className="w-16 text-right text-xs text-text-tertiary font-mono">
                  {formatNumber(w.requests)} req
                </span>
              </div>
            ))}
          </div>
        </TerminalCard>
      </div>
    </div>
  );
}
