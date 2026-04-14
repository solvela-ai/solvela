import { cn } from "@/lib/utils";

type Status = "ok" | "degraded" | "down" | "unknown";

interface StatusDotProps {
  status: Status;
  label?: string;
}

const STATUS_STYLES: Record<Status, string> = {
  ok:       "bg-success",
  degraded: "bg-warning",
  down:     "bg-error",
  unknown:  "bg-text-tertiary",
};

export function StatusDot({ status, label }: StatusDotProps) {
  return (
    <span
      className="inline-flex items-center gap-1.5 text-xs text-text-secondary"
      role="status"
      aria-label={`${label ?? status}: ${status}`}
    >
      <span
        className={cn(
          "inline-block h-2 w-2 rounded-full",
          STATUS_STYLES[status]
        )}
      />
      {label ?? status}
    </span>
  );
}
