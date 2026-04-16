"use client";

import { useRouter } from "next/navigation";
import { RefreshCw } from "lucide-react";

interface TopbarProps {
  title: string;
  subtitle?: string;
}

export function Topbar({ title, subtitle }: TopbarProps) {
  const router = useRouter();
  return (
    <header className="flex items-center justify-between border-b border-border bg-bg-inset px-6 py-4">
      <div>
        <h1 className="metric-md">
          {title}
        </h1>
        {subtitle && (
          <p className="text-xs text-text-tertiary mt-1">{subtitle}</p>
        )}
      </div>
      <div className="flex items-center gap-3">
        <button
          type="button"
          className="flex items-center gap-1.5 rounded border border-border min-h-10 px-3 py-2 text-xs text-text-secondary hover:text-text-primary hover:bg-bg-surface transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-[color:var(--accent-salmon)]"
          onClick={() => router.refresh()}
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
