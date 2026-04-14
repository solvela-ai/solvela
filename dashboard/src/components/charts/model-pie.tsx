"use client";

import {
  PieChart,
  Pie,
  Cell,
  Tooltip,
  ResponsiveContainer,
  Legend,
} from "recharts";
import type { ModelUsage } from "@/types";
import { providerColor } from "@/lib/utils";

interface ModelPieProps {
  data: ModelUsage[];
}

export function ModelPie({ data }: ModelPieProps) {
  return (
    <ResponsiveContainer width="100%" height={220}>
      <PieChart>
        <Pie
          data={data}
          dataKey="pct"
          nameKey="model"
          cx="50%"
          cy="50%"
          innerRadius={55}
          outerRadius={85}
          paddingAngle={2}
        >
          {data.map((entry) => (
            <Cell
              key={entry.model}
              fill={providerColor(entry.provider)}
            />
          ))}
        </Pie>
        <Tooltip
          contentStyle={{
            background: "#1c1c1c",
            border: "1px solid #262626",
            borderRadius: 8,
            fontSize: 12,
          }}
          formatter={(v: number | undefined) => [
            v != null ? `${v}%` : "-",
            "Share",
          ]}
        />
        <Legend
          formatter={(value) => (
            <span style={{ fontSize: 11, color: "#737373" }}>{value}</span>
          )}
        />
      </PieChart>
    </ResponsiveContainer>
  );
}
