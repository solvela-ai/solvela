import { CheckCircle, XCircle, Zap, AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { Badge } from "@/components/ui/badge";
import { StatusDot } from "@/components/ui/status-dot";
import { fetchPricing } from "@/lib/api";
import { providerBadgeClass } from "@/lib/utils";
import type { Model } from "@/types";

function Cap({ on }: { on: boolean }) {
  return on ? (
    <CheckCircle size={14} className="text-success" />
  ) : (
    <XCircle size={14} className="text-text-tertiary" />
  );
}

function ModelRow({ m }: { m: Model }) {
  return (
    <tr className="hover:bg-bg-surface-hover transition-colors">
      <td className="px-5 py-3">
        <div className="font-medium text-text-primary">{m.display_name}</div>
        <div className="text-xs text-text-tertiary font-mono mt-0.5">{m.id}</div>
      </td>
      <td className="px-5 py-3">
        <Badge className={providerBadgeClass(m.provider)}>{m.provider}</Badge>
      </td>
      <td className="px-5 py-3 text-right tabular-nums text-text-secondary">
        ${(m.pricing.input_per_million_usdc * 1.05).toFixed(3)}
      </td>
      <td className="px-5 py-3 text-right tabular-nums text-text-secondary">
        ${(m.pricing.output_per_million_usdc * 1.05).toFixed(3)}
      </td>
      <td className="px-5 py-3 text-center text-text-secondary">
        {m.capabilities.context_window >= 1_000_000
          ? `${(m.capabilities.context_window / 1_000_000).toFixed(0)}M`
          : `${(m.capabilities.context_window / 1_000).toFixed(0)}k`}
      </td>
      <td className="px-5 py-3 text-center">
        <Cap on={m.capabilities.streaming} />
      </td>
      <td className="px-5 py-3 text-center">
        <Cap on={m.capabilities.tools} />
      </td>
      <td className="px-5 py-3 text-center">
        <Cap on={m.capabilities.vision} />
      </td>
      <td className="px-5 py-3 text-center">
        <Cap on={m.capabilities.reasoning} />
      </td>
      <td className="px-5 py-3 text-center">
        {/* Live status from health endpoint — defaulting to ok for now */}
        <StatusDot status="ok" />
      </td>
    </tr>
  );
}

export default async function ModelsPage() {
  let models: Model[] = [];
  let error: string | null = null;

  try {
    const pricing = await fetchPricing();
    models = pricing.models;
  } catch (err) {
    error = err instanceof Error ? err.message : "Failed to load models";
  }

  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Models"
        subtitle={
          models.length > 0
            ? `${models.length} models · 5% platform fee included · live from /pricing`
            : "Model registry"
        }
      />

      <div className="flex-1 p-6">
        {error && (
          <div className="mb-4 flex items-center gap-2 rounded-lg border border-warning/20 bg-warning/10 px-4 py-3 text-sm text-warning">
            <AlertTriangle size={14} className="flex-shrink-0" />
            <span>
              Could not reach gateway ({error}). Start the gateway with{" "}
              <code className="font-mono text-xs">cargo run -p gateway</code>{" "}
              and refresh.
            </span>
          </div>
        )}

        <div className="rounded-xl border border-border bg-bg-surface overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-bg-surface-hover text-xs text-text-secondary uppercase tracking-wide border-b border-border-subtle">
                  <th className="px-5 py-3 text-left font-medium">Model</th>
                  <th className="px-5 py-3 text-left font-medium">Provider</th>
                  <th className="px-5 py-3 text-right font-medium">Input /M tokens</th>
                  <th className="px-5 py-3 text-right font-medium">Output /M tokens</th>
                  <th className="px-5 py-3 text-center font-medium">Context</th>
                  <th className="px-5 py-3 text-center font-medium">
                    <Zap size={12} className="inline" /> Stream
                  </th>
                  <th className="px-5 py-3 text-center font-medium">Tools</th>
                  <th className="px-5 py-3 text-center font-medium">Vision</th>
                  <th className="px-5 py-3 text-center font-medium">Reasoning</th>
                  <th className="px-5 py-3 text-center font-medium">Status</th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border-subtle">
                {models.length > 0 ? (
                  models.map((m) => <ModelRow key={m.id} m={m} />)
                ) : (
                  <tr>
                    <td
                      colSpan={10}
                      className="px-5 py-8 text-center text-sm text-text-tertiary"
                    >
                      {error
                        ? "No model data available — gateway offline"
                        : "No models found"}
                    </td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
        </div>

        <p className="mt-3 text-xs text-text-tertiary">
          Prices in USDC per million tokens including the 5% platform fee.
          Data sourced from{" "}
          <code className="font-mono">GET /pricing</code> on the gateway.
        </p>
      </div>
    </div>
  );
}
