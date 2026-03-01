import { cn } from "@/lib/utils";

type Status = "ok" | "degraded" | "down" | "unknown";

interface StatusDotProps {
  status: Status;
  label?: string;
}

const STATUS_STYLES: Record<Status, string> = {
  ok:       "bg-green-500",
  degraded: "bg-yellow-500",
  down:     "bg-red-500",
  unknown:  "bg-gray-400",
};

export function StatusDot({ status, label }: StatusDotProps) {
  return (
    <span className="inline-flex items-center gap-1.5 text-xs text-gray-600">
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
