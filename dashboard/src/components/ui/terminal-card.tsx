import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export interface TerminalCardProps {
  title: ReactNode;
  meta?: ReactNode;
  accentDot?: boolean;
  bare?: boolean;
  className?: string;
  screenClassName?: string;
  children: ReactNode;
}

export function TerminalCard({
  title,
  meta,
  accentDot = false,
  bare = false,
  className,
  screenClassName,
  children,
}: TerminalCardProps) {
  return (
    <div className={cn("terminal-card", className)}>
      <div className="terminal-card-titlebar">
        <span className="terminal-card-dots" aria-hidden>
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot" />
          <span
            className={cn(
              "terminal-card-dot",
              accentDot && "terminal-card-dot--accent",
            )}
          />
        </span>
        <span className="truncate">{title}</span>
        {meta && <span className="ml-auto">{meta}</span>}
      </div>
      {bare ? (
        children
      ) : (
        <div className={cn("terminal-card-screen", screenClassName)}>
          {children}
        </div>
      )}
    </div>
  );
}
