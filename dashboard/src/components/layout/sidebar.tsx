"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  LayoutDashboard,
  BarChart3,
  Cpu,
  Wallet,
  Settings,
  Zap,
} from "lucide-react";
import { cn } from "@/lib/utils";

const NAV = [
  { href: "/dashboard/overview",  label: "Overview",  icon: LayoutDashboard },
  { href: "/dashboard/usage",     label: "Usage",     icon: BarChart3 },
  { href: "/dashboard/models",    label: "Models",    icon: Cpu },
  { href: "/dashboard/wallet",    label: "Wallet",    icon: Wallet },
  { href: "/dashboard/settings",  label: "Settings",  icon: Settings },
];

interface SidebarProps {
  open?: boolean;
  onClose?: () => void;
}

export function Sidebar({ open, onClose }: SidebarProps) {
  const pathname = usePathname();

  return (
    <>
      {/* Mobile overlay */}
      {open && (
        <div
          className="fixed inset-0 z-30 bg-black/30 md:hidden"
          onClick={onClose}
          aria-hidden
        />
      )}
      <aside
        className={cn(
          "flex h-screen w-56 flex-col border-r border-border bg-bg-surface",
          "max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:transition-transform max-md:duration-200",
          open ? "max-md:translate-x-0" : "max-md:-translate-x-full"
        )}
      >
      {/* Logo */}
      <div className="flex items-center gap-2 px-5 py-5 border-b border-border-subtle">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-brand text-white">
          <Zap size={16} />
        </div>
        <span className="font-semibold text-text-primary text-sm">Solvela</span>
      </div>

      {/* Nav */}
      <nav className="flex-1 px-3 py-4 space-y-0.5">
        {NAV.map(({ href, label, icon: Icon }) => {
          const active = pathname === href || pathname.startsWith(href + "/");
          return (
            <Link
              key={href}
              href={href}
              className={cn(
                "flex items-center gap-3 rounded-lg px-3 py-2 text-sm font-medium transition-colors",
                active
                  ? "bg-brand-subtle text-brand-text"
                  : "text-text-secondary hover:bg-bg-surface-hover hover:text-text-primary"
              )}
            >
              <Icon size={16} />
              {label}
            </Link>
          );
        })}
      </nav>

      {/* Footer */}
      <div className="px-4 py-3 border-t border-border-subtle">
        <p className="text-xs text-text-tertiary">Solana · USDC-SPL · x402 v2</p>
      </div>
    </aside>
    </>
  );
}
