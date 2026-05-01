export const dynamic = 'force-dynamic';

import {
  Activity,
  Zap,
  Cpu,
  Wallet,
  AlertTriangle,
} from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { StatCard } from "@/components/ui/stat-card";
import { TerminalCard } from "@/components/ui/terminal-card";
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
        uniqueWallets: 3,
        activeModels: DASHBOARD_STATS.activeModels,
        cacheHitRate: 0.34,
      };

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

      <div className="flex-1 p-6 space-y-5">
        {/* Mock data warning */}
        {usingMockData && (
          <div role="status" aria-live="polite" className="flex items-center gap-2 rounded border border-border px-4 py-2.5 text-sm text-text-secondary">
            <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
            <span>Gateway offline — showing sample data.</span>
          </div>
        )}

        {/* Stats grid */}
        <div>
          <p className="eyebrow mb-3">30-day metrics</p>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-4 lg:grid-rows-2">
            {/* Hero treasury card — dominant */}
            <TerminalCard
              title="treasury.overview"
              meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
              className="md:col-span-2 lg:row-span-2"
              screenClassName="!p-6"
            >
              <p className="text-xs text-text-tertiary font-mono uppercase tracking-wide mb-2">Total spend</p>
              <p className="metric-xl">{formatUSDC(s.totalSpend, 2)}</p>
              <p className="mt-3 text-xs text-text-tertiary font-mono">
                USDC-SPL · {formatUSDC(s.avgCostPerRequest, 5)} avg per call
              </p>
            </TerminalCard>

            {/* Four supporting stats */}
            <StatCard
              title="Requests"
              value={formatNumber(s.totalRequests)}
              subtitle="API calls"
              icon={Activity}
            />
            <StatCard
              title="Cache Hit"
              value={`${Math.round(s.cacheHitRate * 100)}%`}
              subtitle="response cache"
              icon={Zap}
            />
            <StatCard
              title="Models"
              value={String(s.activeModels)}
              subtitle="active this period"
              icon={Cpu}
            />
            <StatCard
              title="Wallets"
              value={String(s.uniqueWallets)}
              subtitle="unique this period"
              icon={Wallet}
            />
          </div>
        </div>

        {/* Charts */}
        <div>
          <p className="eyebrow mb-3">Activity</p>
          <div className="grid grid-cols-1 lg:grid-cols-2 gap-3">
            <TerminalCard
              title="spend.usdc.daily"
              meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
            >
              <SpendChart data={history} />
            </TerminalCard>

            <TerminalCard
              title="requests.daily"
              meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
            >
              <RequestsBar data={history} />
            </TerminalCard>
          </div>
        </div>

        {/* Gateway status */}
        <TerminalCard
          title="system.health"
          meta={<span className="text-xxs text-text-tertiary font-mono">v{gatewayVersion} · x402 v2</span>}
          screenClassName="!py-5 !px-6"
        >
          <div className="flex flex-wrap gap-6">
            <StatusDot status={gatewayStatus === "ok" ? "ok" : gatewayStatus === "degraded" ? "degraded" : "down"} label="Gateway" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Solana RPC" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="OpenAI" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Anthropic" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="Google" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="xAI" />
            <StatusDot status={gatewayStatus === "ok" ? "ok" : "degraded"} label="DeepSeek" />
          </div>
        </TerminalCard>
      </div>
    </div>
  );
}
