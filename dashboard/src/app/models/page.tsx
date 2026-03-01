import { CheckCircle, XCircle, Zap } from "lucide-react";
import { Topbar } from "@/components/layout/topbar";
import { Badge } from "@/components/ui/badge";
import { StatusDot } from "@/components/ui/status-dot";
import { providerBadgeClass } from "@/lib/utils";

// Static model data (mirrors config/models.toml); live data loaded from /pricing
const MODELS = [
  {
    id: "openai/gpt-5-2",
    name: "GPT-5.2",
    provider: "openai",
    input: 1.75,
    output: 14.0,
    context: "400k",
    streaming: true,
    tools: true,
    vision: true,
    reasoning: true,
    status: "ok" as const,
  },
  {
    id: "openai/gpt-4o",
    name: "GPT-4o",
    provider: "openai",
    input: 2.5,
    output: 10.0,
    context: "128k",
    streaming: true,
    tools: true,
    vision: true,
    reasoning: false,
    status: "ok" as const,
  },
  {
    id: "openai/gpt-4o-mini",
    name: "GPT-4o Mini",
    provider: "openai",
    input: 0.15,
    output: 0.6,
    context: "128k",
    streaming: true,
    tools: true,
    vision: false,
    reasoning: false,
    status: "ok" as const,
  },
  {
    id: "openai/o3",
    name: "o3",
    provider: "openai",
    input: 2.0,
    output: 8.0,
    context: "200k",
    streaming: true,
    tools: false,
    vision: false,
    reasoning: true,
    status: "ok" as const,
  },
  {
    id: "anthropic/claude-sonnet-4",
    name: "Claude Sonnet 4",
    provider: "anthropic",
    input: 3.0,
    output: 15.0,
    context: "200k",
    streaming: true,
    tools: true,
    vision: true,
    reasoning: false,
    status: "ok" as const,
  },
  {
    id: "anthropic/claude-haiku-4",
    name: "Claude Haiku 4",
    provider: "anthropic",
    input: 0.8,
    output: 4.0,
    context: "200k",
    streaming: true,
    tools: true,
    vision: false,
    reasoning: false,
    status: "ok" as const,
  },
  {
    id: "google/gemini-2.5-pro",
    name: "Gemini 2.5 Pro",
    provider: "google",
    input: 1.25,
    output: 5.0,
    context: "1M",
    streaming: true,
    tools: true,
    vision: true,
    reasoning: true,
    status: "ok" as const,
  },
  {
    id: "google/gemini-2.5-flash",
    name: "Gemini 2.5 Flash",
    provider: "google",
    input: 0.075,
    output: 0.3,
    context: "1M",
    streaming: true,
    tools: true,
    vision: true,
    reasoning: false,
    status: "ok" as const,
  },
  {
    id: "xai/grok-3",
    name: "Grok-3",
    provider: "xai",
    input: 3.0,
    output: 15.0,
    context: "131k",
    streaming: true,
    tools: false,
    vision: false,
    reasoning: false,
    status: "degraded" as const,
  },
  {
    id: "deepseek/deepseek-v3",
    name: "DeepSeek V3",
    provider: "deepseek",
    input: 0.27,
    output: 1.1,
    context: "64k",
    streaming: true,
    tools: true,
    vision: false,
    reasoning: false,
    status: "ok" as const,
  },
];

function Cap({ on }: { on: boolean }) {
  return on ? (
    <CheckCircle size={14} className="text-green-500" />
  ) : (
    <XCircle size={14} className="text-gray-300" />
  );
}

export default function ModelsPage() {
  return (
    <div className="flex flex-col h-full">
      <Topbar
        title="Models"
        subtitle={`${MODELS.length} models across 5 providers · 5% platform fee included`}
      />

      <div className="flex-1 p-6">
        <div className="rounded-xl border border-gray-200 bg-white shadow-sm overflow-hidden">
          <div className="overflow-x-auto">
            <table className="w-full text-sm">
              <thead>
                <tr className="bg-gray-50 text-xs text-gray-500 uppercase tracking-wide border-b border-gray-100">
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
              <tbody className="divide-y divide-gray-100">
                {MODELS.map((m) => {
                  // Add 5% platform fee
                  const fee = 1.05;
                  const inputTotal = (m.input * fee).toFixed(3);
                  const outputTotal = (m.output * fee).toFixed(3);
                  return (
                    <tr
                      key={m.id}
                      className="hover:bg-gray-50 transition-colors"
                    >
                      <td className="px-5 py-3">
                        <div className="font-medium text-gray-900">
                          {m.name}
                        </div>
                        <div className="text-xs text-gray-400 font-mono mt-0.5">
                          {m.id}
                        </div>
                      </td>
                      <td className="px-5 py-3">
                        <Badge className={providerBadgeClass(m.provider)}>
                          {m.provider}
                        </Badge>
                      </td>
                      <td className="px-5 py-3 text-right tabular-nums text-gray-700">
                        ${inputTotal}
                      </td>
                      <td className="px-5 py-3 text-right tabular-nums text-gray-700">
                        ${outputTotal}
                      </td>
                      <td className="px-5 py-3 text-center text-gray-500">
                        {m.context}
                      </td>
                      <td className="px-5 py-3 text-center">
                        <Cap on={m.streaming} />
                      </td>
                      <td className="px-5 py-3 text-center">
                        <Cap on={m.tools} />
                      </td>
                      <td className="px-5 py-3 text-center">
                        <Cap on={m.vision} />
                      </td>
                      <td className="px-5 py-3 text-center">
                        <Cap on={m.reasoning} />
                      </td>
                      <td className="px-5 py-3 text-center">
                        <StatusDot status={m.status} />
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        </div>

        <p className="mt-3 text-xs text-gray-400">
          Prices in USDC per million tokens. All prices include the 5% RustyClawRouter platform fee.
          Provider costs fetched from <code className="font-mono">GET /pricing</code>.
        </p>
      </div>
    </div>
  );
}
