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

export function SpendChart({ data }: SpendChartProps) {
  return (
    <ResponsiveContainer width="100%" height={220}>
      <AreaChart data={data} margin={{ top: 4, right: 8, left: 0, bottom: 0 }}>
        <defs>
          <linearGradient id="spendGrad" x1="0" y1="0" x2="0" y2="1">
            <stop offset="5%" stopColor="#f97316" stopOpacity={0.15} />
            <stop offset="95%" stopColor="#f97316" stopOpacity={0} />
          </linearGradient>
        </defs>
        <CartesianGrid strokeDasharray="3 3" stroke="#1c1c1c" />
        <XAxis
          dataKey="date"
          tick={{ fontSize: 11, fill: "#737373" }}
          tickLine={false}
          axisLine={false}
          interval={6}
        />
        <YAxis
          tick={{ fontSize: 11, fill: "#737373" }}
          tickLine={false}
          axisLine={false}
          tickFormatter={(v) => `$${v.toFixed(2)}`}
          width={48}
        />
        <Tooltip
          contentStyle={{
            background: "#1c1c1c",
            border: "1px solid #262626",
            borderRadius: 8,
            fontSize: 12,
          }}
          formatter={(v: number | undefined) => [
            v != null ? `$${v.toFixed(4)} USDC` : "-",
            "Spend",
          ]}
        />
        <Area
          type="monotone"
          dataKey="spend"
          stroke="#f97316"
          strokeWidth={2}
          fill="url(#spendGrad)"
          dot={false}
          activeDot={{ r: 4 }}
        />
      </AreaChart>
    </ResponsiveContainer>
  );
}
