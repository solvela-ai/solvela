import type { ReactNode } from 'react'
import { cn } from '@/lib/utils'

export interface MetricTileProps {
  label: string
  /**
   * The number or string to display. Pass `null` to render the empty-state
   * placeholder ("—") which signals "we don't have this datapoint yet"
   * rather than the misleading literal "0".
   */
  value: ReactNode | null
  /** Smaller line below the value: unit, scope, etc. */
  subtitle?: string
  /** Visual emphasis. The "primary" tile gets larger type and accent rule. */
  emphasis?: 'primary' | 'default'
  className?: string
}

const EMPTY = (
  <span className="text-text-faint" aria-label="not available">
    —
  </span>
)

export function MetricTile({
  label,
  value,
  subtitle,
  emphasis = 'default',
  className,
}: MetricTileProps) {
  const isPrimary = emphasis === 'primary'
  return (
    <div
      className={cn(
        'flex flex-col gap-2 bg-[var(--background)] px-5 py-5 sm:py-6',
        isPrimary && 'sm:border-l-2 sm:border-[var(--accent-salmon)] sm:pl-6',
        className,
      )}
    >
      <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
        {label}
      </span>
      <span className={isPrimary ? 'metric-xl' : 'metric-lg'}>
        {value ?? EMPTY}
      </span>
      {subtitle && (
        <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint">
          {subtitle}
        </span>
      )}
    </div>
  )
}
