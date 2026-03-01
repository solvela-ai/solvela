import { Topbar } from "@/components/layout/topbar";
import { ModelPie } from "@/components/charts/model-pie";
import { SpendChart } from "@/components/charts/spend-chart";
import { Badge } from "@/components/ui/badge";
import { MODEL_USAGE, generateSpendHistory } from "@/lib/mock-data";
import {
  formatUSDC,
  formatNumber,
  providerBadgeClass,
} from "@/lib/utils";

export default function UsagePage() {
  const history = generateSpendHistory();

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Usage"
        subtitle="Per-model and per-wallet breakdown"
      />

      <div className="flex-1 p-6 space-y-6">
        {/* Top row: pie + spend line */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
            <h2 className="text-sm font-semibold text-gray-900 mb-4">
              Requests by Model
            </h2>
            <ModelPie data={MODEL_USAGE} />
          </div>

          <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
            <h2 className="text-sm font-semibold text-gray-900 mb-4">
              Spend Over Time
            </h2>
            <SpendChart data={history} />
          </div>
        </div>

        {/* Model breakdown table */}
        <div className="rounded-xl border border-gray-200 bg-white shadow-sm overflow-hidden">
          <div className="px-5 py-4 border-b border-gray-100">
            <h2 className="text-sm font-semibold text-gray-900">
              Model Breakdown
            </h2>
          </div>
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-gray-50 text-xs text-gray-500 uppercase tracking-wide">
                  <th className="px-5 py-3 text-left font-medium">Model</th>
                  <th className="px-5 py-3 text-left font-medium">Provider</th>
                  <th className="px-5 py-3 text-right font-medium">Requests</th>
                  <th className="px-5 py-3 text-right font-medium">Spend (USDC)</th>
                  <th className="px-5 py-3 text-right font-medium">Share</th>
                  <th className="px-5 py-3 text-left font-medium">Distribution</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-gray-100">
                {MODEL_USAGE.map((row) => (
                  <tr key={row.model} className="hover:bg-gray-50 transition-colors">
                    <td className="px-5 py-3 font-medium text-gray-900">
                      {row.model}
                    </td>
                    <td className="px-5 py-3">
                      <Badge className={providerBadgeClass(row.provider)}>
                        {row.provider}
                      </Badge>
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-gray-700">
                      {formatNumber(row.requests)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-gray-700">
                      {formatUSDC(row.spend, 4)}
                    </td>
                    <td className="px-5 py-3 text-right tabular-nums text-gray-700">
                      {row.pct}%
                    </td>
                    <td className="px-5 py-3">
                      <div className="h-1.5 w-32 rounded-full bg-gray-100">
                        <div
                          className="h-1.5 rounded-full bg-orange-400"
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
        <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-4">
            Top Wallets by Spend
          </h2>
          <div className="space-y-3">
            {[
              { wallet: "7xKpF...mR9t", spend: 0.62, requests: 412, pct: 55 },
              { wallet: "3yLqZ...nS2v", spend: 0.31, requests: 278, pct: 27 },
              { wallet: "9bNwA...8kJx", spend: 0.20, requests: 237, pct: 18 },
            ].map((w) => (
              <div key={w.wallet} className="flex items-center gap-4">
                <code className="w-28 text-xs text-gray-500 font-mono">
                  {w.wallet}
                </code>
                <div className="flex-1 h-1.5 rounded-full bg-gray-100">
                  <div
                    className="h-1.5 rounded-full bg-blue-400"
                    style={{ width: `${w.pct}%` }}
                  />
                </div>
                <span className="w-20 text-right text-xs tabular-nums text-gray-700">
                  {formatUSDC(w.spend, 2)}
                </span>
                <span className="w-16 text-right text-xs text-gray-400">
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
