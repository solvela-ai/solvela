import {
  DollarSign,
  Activity,
  TrendingDown,
  Zap,
  Cpu,
  Wallet,
  AlertTriangle,
} from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatCard } from "@/components/ui/stat-card";
import { SpendChart } from "@/components/charts/spend-chart";
import { RequestsBar } from "@/components/charts/requests-bar";
import { StatusDot } from "@/components/ui/status-dot";
import { DASHBOARD_STATS, SPEND_HISTORY } from "@/lib/mock-data";
import { fetchAdminStats, fetchHealth } from "@/lib/api";
import { formatUSDC, formatNumber } from "@/lib/utils";
import type { SpendDataPoint } from "@/types";

export default async function OverviewPage() {
  const [statsResponse, healthResponse] = await Promise.all([
    fetchAdminStats(30),
    fetchHealth().catch((error) => {
      console.error("[OverviewPage] Health check failed:", error);
      return null;
    }),
  ]);

  const usingMockData = !statsResponse;

  // Map API data to display values, falling back to mock data
  const s = statsResponse
    ? {
        totalSpend: parseFloat(statsResponse.summary.total_cost_usdc),
        totalRequests: statsResponse.summary.total_requests,
        avgCostPerRequest:
          statsResponse.summary.total_requests > 0
            ? parseFloat(statsResponse.summary.total_cost_usdc) /
              statsResponse.summary.total_requests
            : 0,
        uniqueWallets: statsResponse.summary.unique_wallets,
        activeModels: statsResponse.by_model.length,
        cacheHitRate: statsResponse.summary.cache_hit_rate ?? 0,
      }
    : {
        totalSpend: DASHBOARD_STATS.totalSpend,
        totalRequests: DASHBOARD_STATS.totalRequests,
        avgCostPerRequest: DASHBOARD_STATS.avgCostPerRequest,
        uniqueWallets: 0,
        activeModels: DASHBOARD_STATS.activeModels,
        cacheHitRate: 0,
      };

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

  const gatewayStatus = healthResponse?.status ?? "down";
  const gatewayVersion = healthResponse?.version ?? "unknown";

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Overview"
        subtitle="Last 30 days · All models · Solana mainnet"
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
            title="Cache Hit Rate"
            value={`${Math.round(s.cacheHitRate * 100)}%`}
            subtitle="response cache"
            icon={Zap}
            iconColor="text-success"
          />
          <StatCard
            title="Active Models"
            value={String(s.activeModels)}
            subtitle="in use this period"
            icon={Cpu}
          />
          <StatCard
            title="Unique Wallets"
            value={String(s.uniqueWallets)}
            subtitle="this period"
            icon={Wallet}
            iconColor="text-info"
          />
        </div>

        {/* Charts row */}
        <div className="grid grid-cols-1 lg:grid-cols-2 gap-4">
          <div className="rounded-xl border border-border bg-bg-surface p-5">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-sm font-semibold text-text-primary">
                Daily Spend (USDC)
              </h2>
              <span className="text-xs text-text-tertiary">30 days</span>
            </div>
            <SpendChart data={history} />
          </div>

          <div className="rounded-xl border border-border bg-bg-surface p-5">
            <div className="flex items-center justify-between mb-4">
              <h2 className="text-sm font-semibold text-text-primary">
                Daily Requests
              </h2>
              <span className="text-xs text-text-tertiary">30 days</span>
            </div>
            <RequestsBar data={history} />
          </div>
        </div>

        {/* Gateway status */}
        <div className="rounded-xl border border-border bg-bg-surface p-5">
          <h2 className="text-sm font-semibold text-text-primary mb-3">
            Gateway Status
          </h2>
          <div className="flex flex-wrap gap-6 text-sm">
            <div className="flex items-center gap-2">
              <StatusDot status={gatewayStatus === "ok" ? "ok" : gatewayStatus === "degraded" ? "degraded" : "down"} label="Gateway" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Solana RPC" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="OpenAI" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Anthropic" />
            </div>
            <div className="flex items-center gap-2">
              <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Google" />
            </div>
            <div className="ml-auto text-xs text-text-tertiary">
              v{gatewayVersion} · x402 v2 · Solana mainnet
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
