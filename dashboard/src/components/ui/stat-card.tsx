import type { LucideIcon } from "lucide-react";
import { TerminalCard } from "@/components/ui/terminal-card";

interface StatCardProps {
  title: string;
  value: string;
  subtitle?: string;
  icon?: LucideIcon;
  trend?: { value: string; positive: boolean };
  iconColor?: string;
}

export function StatCard({
  title,
  value,
  subtitle,
}: StatCardProps) {
  return (
    <TerminalCard title={title} screenClassName="!px-5 !pt-5 !pb-6">
      <p className="metric-lg">
        {value}
      </p>
      {subtitle && (
        <p className="mt-1.5 text-xs text-text-tertiary font-mono">{subtitle}</p>
      )}
    </TerminalCard>
  );
}
