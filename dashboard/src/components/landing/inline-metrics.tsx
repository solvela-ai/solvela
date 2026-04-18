import { CountUp } from './count-up'
import { METRICS } from './config'

export function InlineMetrics() {
  return (
    <div className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-[var(--border)] sm:grid-cols-4">
      {METRICS.map((m) => (
        <div
          key={m.label}
          className="flex flex-col gap-2 bg-[var(--background)] px-5 py-5 sm:py-6"
        >
          <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
            {m.label}
          </span>
          <CountUp
            value={m.value}
            decimals={m.decimals}
            suffix={m.suffix}
            className="metric-lg"
          />
        </div>
      ))}
    </div>
  )
}
