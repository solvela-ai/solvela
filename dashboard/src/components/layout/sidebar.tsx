"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import {
  LayoutDashboard,
  BarChart3,
  Cpu,
  Wallet,
  Settings,
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
          "flex h-screen w-56 flex-col border-r border-border bg-bg-inset",
          "max-md:fixed max-md:inset-y-0 max-md:left-0 max-md:z-40 max-md:transition-transform max-md:duration-200",
          open ? "max-md:translate-x-0" : "max-md:-translate-x-full"
        )}
      >
        {/* Logo */}
        <div className="flex items-center gap-2.5 px-5 py-5 border-b border-border">
          <div className="flex h-7 w-7 flex-shrink-0 items-center justify-center rounded border border-border text-text-primary text-sm font-bold font-display">
            S
          </div>
          <span className="font-semibold text-text-primary text-sm font-display tracking-tight">
            Solvela
          </span>
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
                    ? "nav-item-active text-text-primary bg-bg-surface"
                    : "text-text-secondary hover:bg-bg-surface hover:text-text-primary"
                )}
              >
                <Icon size={15} />
                {label}
              </Link>
            );
          })}
        </nav>

        {/* Footer */}
        <div className="px-4 py-3 border-t border-border">
          <p className="text-xs text-text-tertiary font-mono">Solana · USDC-SPL · x402 v2</p>
        </div>
      </aside>
    </>
  );
}
