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

export function RequestsBar({ data }: RequestsBarProps) {
  return (
    <ResponsiveContainer width="100%" height={220}>
      <BarChart
        data={data}
        margin={{ top: 4, right: 8, left: 0, bottom: 0 }}
      >
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
          width={32}
        />
        <Tooltip
          contentStyle={{
            background: "#1c1c1c",
            border: "1px solid #262626",
            borderRadius: 8,
            fontSize: 12,
          }}
          formatter={(v: number | undefined) => [
            v != null ? v : "-",
            "Requests",
          ]}
        />
        <Bar dataKey="requests" fill="#f97316" radius={[3, 3, 0, 0]} />
      </BarChart>
    </ResponsiveContainer>
  );
}
