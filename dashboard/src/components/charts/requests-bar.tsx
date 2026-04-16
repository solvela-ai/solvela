"use client";

import {
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { SpendDataPoint } from "@/types";

interface RequestsBarProps {
  data: SpendDataPoint[];
}

// Literal token values — Recharts props don't resolve CSS variables at runtime
const GRID_STROKE    = "#C8A24030"; // --border
const TICK_COLOR     = "#DEDCD180"; // --color-text-tertiary
const TOOLTIP_BG     = "#30302E";   // --card
const TOOLTIP_BORDER = "#C8A24030"; // --border
const BAR_COLOR      = "#C8A24060"; // --color-border-emphasis (warm gold, muted)

export function RequestsBar({ data }: RequestsBarProps) {
  return (
    <ResponsiveContainer width="100%" height={200}>
      <BarChart data={data} margin={{ top: 4, right: 4, left: 0, bottom: 0 }}>
        <CartesianGrid strokeDasharray="3 3" stroke={GRID_STROKE} vertical={false} />
        <XAxis
          dataKey="date"
          tick={{ fontSize: 10, fill: TICK_COLOR, fontFamily: "JetBrains Mono" }}
          tickLine={false}
          axisLine={false}
          interval={6}
        />
        <YAxis
          tick={{ fontSize: 10, fill: TICK_COLOR, fontFamily: "JetBrains Mono" }}
          tickLine={false}
          axisLine={false}
          width={32}
        />
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
            v != null ? v.toLocaleString() : "—",
            "Requests",
          ]}
        />
        <Bar dataKey="requests" fill={BAR_COLOR} radius={[2, 2, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
