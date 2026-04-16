"use client";

import { RefreshCw } from "lucide-react";

interface TopbarProps {
  title: string;
  subtitle?: string;
}

export function Topbar({ title, subtitle }: TopbarProps) {
  return (
    <header className="flex items-center justify-between border-b border-border bg-bg-inset px-6 py-4">
      <div>
        <p className="eyebrow mb-1" style={{ fontSize: '11px' }}>{title.toUpperCase()}</p>
        <h1 style={{ fontFamily: 'var(--font-serif)', fontWeight: 500, fontSize: '22px', lineHeight: 1.15, letterSpacing: '-0.01em', color: 'var(--heading-color)' }}>
          {title}
        </h1>
        {subtitle && (
          <p className="text-xs text-text-tertiary mt-0.5">{subtitle}</p>
        )}
      </div>
      <div className="flex items-center gap-3">
        <button
          className="flex items-center gap-1.5 rounded border border-border px-2.5 py-1.5 text-xs text-text-secondary hover:text-text-primary hover:bg-bg-surface transition-colors"
          onClick={() => window.location.reload()}
        >
          <RefreshCw size={11} />
          Refresh
        </button>
        <div
          className="h-7 w-7 rounded border border-border flex items-center justify-center text-text-secondary text-xs font-bold font-display"
          aria-label="User account"
        >
          W
        </div>
      </div>
    </header>
  );
}
