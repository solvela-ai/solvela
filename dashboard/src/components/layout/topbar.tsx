"use client";

import { Bell, RefreshCw } from "lucide-react";

interface TopbarProps {
  title: string;
  subtitle?: string;
}

export function Topbar({ title, subtitle }: TopbarProps) {
  return (
    <header className="flex items-center justify-between border-b border-border bg-bg-surface px-6 py-4">
      <div>
        <h1 className="text-lg font-semibold text-text-primary">{title}</h1>
        {subtitle && <p className="text-sm text-text-secondary mt-0.5">{subtitle}</p>}
      </div>
      <div className="flex items-center gap-3">
        <button
          className="flex items-center gap-1.5 rounded-lg border border-border px-3 py-1.5 text-xs text-text-secondary hover:bg-bg-surface-hover transition-colors"
          onClick={() => window.location.reload()}
        >
          <RefreshCw size={12} />
          Refresh
        </button>
        <button aria-label="Notifications" className="relative rounded-lg border border-border p-1.5 text-text-secondary hover:bg-bg-surface-hover transition-colors">
          <Bell size={16} />
          <span className="absolute -top-0.5 -right-0.5 h-2 w-2 rounded-full bg-brand" />
        </button>
        <div className="h-8 w-8 rounded-full bg-gradient-to-br from-orange-400 to-orange-600 flex items-center justify-center text-white text-xs font-bold">
          W
        </div>
      </div>
    </header>
  );
}
