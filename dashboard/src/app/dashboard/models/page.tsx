import { CheckCircle, XCircle, AlertTriangle } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { Badge } from "@/components/ui/badge";
import { StatusDot } from "@/components/ui/status-dot";
import { fetchPricing } from "@/lib/api";
import { providerBadgeClass } from "@/lib/utils";
import type { Model } from "@/types";

function Cap({ on }: { on: boolean }) {
  return on ? (
    <CheckCircle size={13} className="text-success" />
  ) : (
    <XCircle size={13} className="text-text-tertiary" />
  );
}

function ModelRow({ m }: { m: Model }) {
  return (
    <tr className="border-b border-border last:border-0 hover:bg-bg-surface transition-colors">
      <td className="px-5 py-3">
        <div className="font-medium text-text-primary text-sm">{m.display_name}</div>
        <div className="text-xs text-text-tertiary font-mono mt-0.5">{m.id}</div>
      </td>
      <td className="px-5 py-3">
        <Badge className={providerBadgeClass(m.provider)}>{m.provider}</Badge>
      </td>
      <td className="px-5 py-3 text-right tabular-nums text-text-secondary text-xs font-mono">
        ${(m.pricing.input_per_million_usdc * 1.05).toFixed(3)}
      </td>
      <td className="px-5 py-3 text-right tabular-nums text-text-secondary text-xs font-mono">
        ${(m.pricing.output_per_million_usdc * 1.05).toFixed(3)}
      </td>
      <td className="px-5 py-3 text-center text-text-secondary text-xs font-mono">
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

      <div className="flex-1 p-6 space-y-4">
        {error && (
          <div className="flex items-center gap-2 rounded border border-border px-4 py-2.5 text-sm text-text-secondary">
            <AlertTriangle size={13} className="flex-shrink-0 text-warning" />
            <span>
              Could not reach gateway ({error}). Run{" "}
              <code className="font-mono text-xs">cargo run -p gateway</code>{" "}
              and refresh.
            </span>
          </div>
        )}

        {/* Models table */}
        <div className="terminal-card overflow-hidden">
          <div className="terminal-card-titlebar">
            <span className="terminal-card-dots">
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
              <span className="terminal-card-dot" />
            </span>
            <span>model.registry</span>
          </div>
          <div className="overflow-x-auto" style={{ background: 'var(--popover)' }}>
            <table className="w-full text-sm">
              <thead>
                <tr className="border-b border-border text-xs text-text-tertiary uppercase tracking-wide font-mono">
                  <th className="px-5 py-2.5 text-left font-medium">Model</th>
                  <th className="px-5 py-2.5 text-left font-medium">Provider</th>
                  <th className="px-5 py-2.5 text-right font-medium">In /M</th>
                  <th className="px-5 py-2.5 text-right font-medium">Out /M</th>
                  <th className="px-5 py-2.5 text-center font-medium">Context</th>
                  <th className="px-5 py-2.5 text-center font-medium">Stream</th>
                  <th className="px-5 py-2.5 text-center font-medium">Tools</th>
                  <th className="px-5 py-2.5 text-center font-medium">Vision</th>
                  <th className="px-5 py-2.5 text-center font-medium">Reason</th>
                  <th className="px-5 py-2.5 text-center font-medium">Status</th>
                </tr>
              </thead>
              <tbody>
                {models.length > 0 ? (
                  models.map((m) => <ModelRow key={m.id} m={m} />)
                ) : (
                  <tr>
                    <td
                      colSpan={10}
                      className="px-5 py-10 text-center text-sm text-text-tertiary font-mono"
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

        <p className="text-xs text-text-tertiary font-mono">
          Prices in USDC per million tokens including the 5% platform fee.
          Source:{" "}
          <code>GET /pricing</code>
        </p>
      </div>
    </div>
  );
}
