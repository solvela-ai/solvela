import {
  DollarSign,
  Activity,
  TrendingDown,
  Zap,
  Cpu,
  Wallet,
} from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatCard } from "@/components/ui/stat-card";
import { SpendChart } from "@/components/charts/spend-chart";
import { RequestsBar } from "@/components/charts/requests-bar";
import { StatusDot } from "@/components/ui/status-dot";
import { DASHBOARD_STATS, generateSpendHistory } from "@/lib/mock-data";
import { formatUSDC, formatNumber } from "@/lib/utils";

export default function OverviewPage() {
  const history = generateSpendHistory();
  const s = DASHBOARD_STATS;

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Overview"
        subtitle="Last 30 days · All models · Solana mainnet"
      />

      <div className="flex-1 p-6 space-y-6">
        {/* Stats row */}
        <div className="grid grid-cols-2 lg:grid-cols-3 xl:grid-cols-6 gap-4">
          <StatCard
            title="Total Spend"
            value={formatUSDC(s.totalSpend, 2)}
            subtitle="USDC-SPL"
            icon={DollarSign}
            trend={{ value: "12%", positive: false }}
          />
          <StatCard
            title="Requests"
            value={formatNumber(s.totalRequests)}
            subtitle="API calls"
            icon={Activity}
            trend={{ value: "8%", positive: true }}
          />
          <StatCard
            title="Avg Cost"
            value={formatUSDC(s.avgCostPerRequest, 5)}
            subtitle="per request"
            icon={TrendingDown}
          />
          <StatCard
            title="Savings vs API"
            value={`${s.savingsVsOpenAI}%`}
            subtitle="vs direct OpenAI"
            icon={Zap}
            iconColor="text-green-600"
          />
          <StatCard
            title="Active Models"
            value={String(s.activeModels)}
            subtitle="in use this period"
            icon={Cpu}
          />
          <StatCard
            title="Wallet Balance"
            value={formatUSDC(s.walletBalance, 2)}
            subtitle="USDC available"
            icon={Wallet}
            iconColor="text-blue-600"
          />
        </div>

        {/* Charts row */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-sm font-semibold text-gray-900">
                Daily Spend (USDC)
              </h2>
              <span className="text-xs text-gray-400">30 days</span>
            </div>
            <SpendChart data={history} />
          </div>

          <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-sm font-semibold text-gray-900">
                Daily Requests
              </h2>
              <span className="text-xs text-gray-400">30 days</span>
            </div>
            <RequestsBar data={history} />
          </div>
        </div>

        {/* Gateway status */}
        <div className="rounded-xl border border-gray-200 bg-white p-5 shadow-sm">
          <h2 className="text-sm font-semibold text-gray-900 mb-3">
            Gateway Status
          </h2>
          <div className="flex flex-wrap gap-6 text-sm">
            <div className="flex items-center gap-2">
              <StatusDot status="ok" label="Gateway" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status="ok" label="Solana RPC" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status="ok" label="OpenAI" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status="ok" label="Anthropic" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status="ok" label="Google" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status="degraded" label="xAI" />
            </div>
            <div className="ml-auto text-xs text-gray-400">
              v0.1.0 · x402 v2 · Solana mainnet
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
