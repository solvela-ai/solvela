import type { LucideIcon } from "lucide-react";

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
    <div className="terminal-card">
      <div className="terminal-card-titlebar">
        <span className="terminal-card-dots">
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot" />
        </span>
        <span className="truncate">{title}</span>
      </div>
      <div className="terminal-card-screen" style={{ padding: '1.25rem 1.25rem 1.5rem' }}>
        <p
          className="tabular-nums leading-none"
          style={{
            fontFamily: 'var(--font-serif)',
            fontSize: '28px',
            fontWeight: 500,
            color: 'var(--heading-color)',
            letterSpacing: '-0.02em',
          }}
        >
          {value}
        </p>
        {subtitle && (
          <p className="mt-1.5 text-xs text-text-tertiary font-mono">{subtitle}</p>
        )}
      </div>
    </div>
  );
}
