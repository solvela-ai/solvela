"use client";

import {
  AreaChart,
  Area,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  ResponsiveContainer,
} from "recharts";
import type { SpendDataPoint } from "@/types";

interface SpendChartProps {
  data: SpendDataPoint[];
}

// Literal token values — Recharts props don't resolve CSS variables at runtime
const GRID_STROKE   = "#C8A24030"; // --border
const TICK_COLOR    = "#DEDCD180"; // --color-text-tertiary
const TOOLTIP_BG    = "#30302E";   // --card
const TOOLTIP_BORDER = "#C8A24030"; // --border
const LINE_COLOR    = "#DEDCD1";   // --foreground (neutral, not brand orange)
const AREA_COLOR    = "#DEDCD1";

export function SpendChart({ data }: SpendChartProps) {
  return (
    <ResponsiveContainer width="100%" height={200}>
      <AreaChart data={data} margin={{ top: 4, right: 4, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="spendGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%"  stopColor={AREA_COLOR} stopOpacity={0.08} />
            <stop offset="95%" stopColor={AREA_COLOR} stopOpacity={0} />
          </linearGradient>
        </defs>
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
          tickFormatter={(v) => `$${v.toFixed(0)}`}
          width={40}
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
            v != null ? `$${v.toFixed(2)} USDC` : "—",
            "Spend",
          ]}
        />
        <Area
          type="monotone"
          dataKey="spend"
          stroke={LINE_COLOR}
          strokeWidth={1.5}
          fill="url(#spendGrad)"
          dot={false}
          activeDot={{ r: 3, fill: LINE_COLOR }}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}
