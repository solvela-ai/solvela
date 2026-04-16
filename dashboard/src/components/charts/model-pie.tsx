"use client";

import {
  PieChart,
  Pie,
  Cell,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { ModelUsage } from "@/types";

interface ModelPieProps {
  data: ModelUsage[];
}

// Warm neutral palette — top model gets the lightest tone, others graduate darker.
// No brand orange; no neon. Just tonal steps through the palette.
const PIE_COLORS = [
  "#DEDCD1", // foreground — top model, most prominent
  "#AEACA4",
  "#8E8C85",
  "#6E6C66",
  "#565450",
  "#3A3936",
  "#30302E",
  "#262624",
];

const TOOLTIP_BG     = "#30302E";
const TOOLTIP_BORDER = "#C8A24030";

export function ModelPie({ data }: ModelPieProps) {
  return (
    <div className="flex items-center gap-4">
      <ResponsiveContainer width={180} height={180}>
        <PieChart>
          <Pie
            data={data}
            dataKey="pct"
            nameKey="model"
            cx="50%"
            cy="50%"
            innerRadius={50}
            outerRadius={80}
            paddingAngle={2}
            strokeWidth={0}
          >
            {data.map((entry, index) => (
              <Cell
                key={entry.model}
                fill={PIE_COLORS[index % PIE_COLORS.length]}
              />
            ))}
          </Pie>
          <Tooltip
            contentStyle={{
              background: TOOLTIP_BG,
              border: `1px solid ${TOOLTIP_BORDER}`,
              borderRadius: "0.375rem",
              fontSize: 11,
              fontFamily: "JetBrains Mono",
              color: "#DEDCD1",
            }}
            formatter={(v: number | undefined) => [
              v != null ? `${v}%` : "—",
              "Share",
            ]}
          />
        </PieChart>
      </ResponsiveContainer>

      {/* Custom legend — tighter, monospaced, no Recharts Legend component */}
      <div className="flex flex-col gap-1.5 min-w-0">
        {data.map((entry, index) => (
          <div key={entry.model} className="flex items-center gap-2 min-w-0">
            <span
              className="inline-block h-2 w-2 rounded-full flex-shrink-0"
              style={{ background: PIE_COLORS[index % PIE_COLORS.length] }}
            />
            <span className="text-xs text-text-secondary font-mono truncate">
              {entry.model}
            </span>
            <span className="text-xs text-text-tertiary font-mono ml-auto pl-2 flex-shrink-0">
              {entry.pct}%
            </span>
          </div>
        ))}
      </div>
    </div>
  );
}
