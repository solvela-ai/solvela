import { CountUp } from './count-up'
import { METRICS } from './config'

// Asymmetric widths break the "identical-tile grid" template. The first
// tile (uptime) gets the most room; the last (platform fee) is compact.
export function InlineMetrics() {
  return (
    <div
      className="grid grid-cols-2 gap-px overflow-hidden rounded-lg border border-border bg-[var(--border)] sm:grid-cols-[1.2fr_1fr_1fr_0.9fr]"
    >
      {METRICS.map((m, i) => (
        <div
          key={m.label}
          className={
            'flex flex-col gap-2 bg-[var(--background)] px-5 py-5 sm:py-6' +
            (i === 0 ? ' sm:border-l-2 sm:border-[var(--accent-salmon)] sm:pl-6' : '')
          }
        >
          <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
            {m.label}
          </span>
          <CountUp
            value={m.value}
            decimals={m.decimals}
            suffix={m.suffix}
            className={i === 0 ? 'metric-xl' : 'metric-lg'}
          />
        </div>
      ))}
    </div>
  )
}
