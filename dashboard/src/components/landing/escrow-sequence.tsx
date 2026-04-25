'use client'

// State machine + layout for the animated escrow explainer.
// Pairs a 5-row timeline (left rail) with <EscrowDiagramAnimated> (right).
// The state machine is deliberately simple: one setTimeout chain, paused when
// out-of-view or when reduced-motion is preferred. After LOOPS_BEFORE_HOLD
// full cycles we stop at the resolved state and expose a manual replay button.

import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import {
  EscrowDiagramAnimated,
  type EscrowBeat,
} from './escrow-diagram-animated'

// Milliseconds spent on each beat before advancing. Total loop ≈ 6.2s.
const BEAT_MS: Record<EscrowBeat, number> = {
  0: 600,   // idle — breathing room before the cycle starts
  1: 1400,  // deposit: agent → escrow
  2: 1200,  // stream: provider → agent
  3: 1000,  // claim + refund, same tx
  4: 2000,  // resolved — hold so viewers can read the final balances
}

const LOOPS_BEFORE_HOLD = 3

interface TimelineRow {
  beat: EscrowBeat
  lead: string
  detail: string
  pair?: 'top' | 'bottom'
  tone: 'neutral' | 'gold' | 'salmon' | 'done'
}

const ROWS: TimelineRow[] = [
  { beat: 1, lead: '0.0042', detail: 'deposit → escrow', tone: 'neutral' },
  { beat: 2, lead: 'stream',  detail: 'provider → agent', tone: 'neutral' },
  { beat: 3, lead: '0.0038', detail: 'claim → provider',  tone: 'gold',   pair: 'top' },
  { beat: 3, lead: '0.0004', detail: 'refund → agent',    tone: 'salmon', pair: 'bottom' },
  { beat: 4, lead: 'delivered', detail: 'escrow settled', tone: 'done' },
]

export function EscrowSequence() {
  const [beat, setBeat] = useState<EscrowBeat>(0)
  const [loopCount, setLoopCount] = useState(0)
  const [isHolding, setIsHolding] = useState(false)
  // Start false — the escrow section is below the fold. IntersectionObserver
  // flips this to true once the user scrolls the section into view, at which
  // point the state machine begins advancing beats. Prevents the common bug
  // where the animation "pre-plays" before the user can see it.
  const [inView, setInView] = useState(false)
  const [reducedMotion, setReducedMotion] = useState(false)
  const wrapperRef = useRef<HTMLDivElement>(null)

  // Pick up reduced-motion preference once on mount.
  useEffect(() => {
    const mq = window.matchMedia('(prefers-reduced-motion: reduce)')
    if (mq.matches) {
      setReducedMotion(true)
      setBeat(4)
      setIsHolding(true)
    }
  }, [])

  // Pause when scrolled out of view — resume from current beat, don't reset.
  useEffect(() => {
    const node = wrapperRef.current
    if (!node) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setInView(e.isIntersecting)
      },
      { rootMargin: '200px 0px' }
    )
    io.observe(node)
    return () => io.disconnect()
  }, [])

  // Advance beats. A single timeout per beat; effect cleans up on state change.
  useEffect(() => {
    if (reducedMotion || !inView || isHolding) return

    const id = setTimeout(() => {
      if (beat === 4) {
        const nextLoop = loopCount + 1
        if (nextLoop >= LOOPS_BEFORE_HOLD) {
          setIsHolding(true)
        } else {
          setLoopCount(nextLoop)
          setBeat(0)
        }
      } else {
        setBeat(((beat + 1) as EscrowBeat))
      }
    }, BEAT_MS[beat])

    return () => clearTimeout(id)
  }, [beat, inView, isHolding, loopCount, reducedMotion])

  const replay = () => {
    setBeat(0)
    setLoopCount(0)
    setIsHolding(false)
  }

  return (
    <div ref={wrapperRef} className="mx-auto max-w-[1280px]">
      <div className="grid gap-0 lg:grid-cols-[minmax(0,0.4fr)_minmax(0,0.6fr)]">
        {/* Left rail — heading + live timeline + tags */}
        <div className="flex flex-col gap-6 border-b border-border/60 px-6 py-16 lg:border-b-0 lg:border-r lg:py-28">
          <span className="eyebrow">the diamond</span>
          <h2
            id="escrow-heading"
            className="font-display text-foreground leading-[0.98] tracking-[-0.02em]"
            style={{ fontSize: 'clamp(2.25rem, 4.2vw, 3.75rem)', fontWeight: 600 }}
          >
            Pay only for
            <br />
            what gets
            <br />
            <span className="font-black" style={{ fontWeight: 900 }}>
              delivered.
            </span>
          </h2>

          <Timeline
            activeBeat={beat}
            isHolding={isHolding}
            rows={ROWS}
          />

          {isHolding && !reducedMotion && (
            <button
              type="button"
              onClick={replay}
              aria-label="replay escrow animation"
              className="inline-flex h-9 w-fit items-center gap-1.5 rounded-md border border-border px-3 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint transition-colors hover:border-[var(--accent-salmon)] hover:text-foreground"
            >
              <span aria-hidden className="text-[14px] leading-none">↻</span>
              replay
            </button>
          )}

          <div className="mt-2 flex flex-wrap items-center gap-3 font-mono text-[11px] uppercase tracking-[0.16em] text-text-faint">
            <span className="rounded-md border border-border px-2 py-1">
              mainnet · anchor
            </span>
            <span className="rounded-md border border-border px-2 py-1">
              usdc-spl
            </span>
            <span className="rounded-md border border-border px-2 py-1">
              audited flows
            </span>
          </div>

          <p className="max-w-[44ch] font-mono text-[11px] leading-[1.6] text-text-faint">
            No other x402 LLM gateway has trustless on-chain escrow.
          </p>
        </div>

        {/* Right — animated diagram bleeds to the right edge */}
        <div className="relative min-h-[460px] overflow-hidden bg-[var(--background)] bg-grid-dense">
          <div className="absolute inset-0 flex items-center justify-center p-6 lg:p-10">
            <div className="h-full w-full max-w-[780px]">
              <EscrowDiagramAnimated beat={beat} loopKey={loopCount} />
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}

function Timeline({
  rows,
  activeBeat,
  isHolding,
}: {
  rows: TimelineRow[]
  activeBeat: EscrowBeat
  isHolding: boolean
}) {
  return (
    <ol
      className="relative m-0 flex list-none flex-col gap-0 p-0"
      aria-label="escrow payment steps"
    >
      {rows.map((row, idx) => (
        <TimelineRowItem
          key={idx}
          row={row}
          activeBeat={activeBeat}
          isHolding={isHolding}
        />
      ))}
    </ol>
  )
}

function TimelineRowItem({
  row,
  activeBeat,
  isHolding,
}: {
  row: TimelineRow
  activeBeat: EscrowBeat
  isHolding: boolean
}) {
  const isActive = !isHolding && activeBeat === row.beat
  const isPast = activeBeat > row.beat || isHolding
  const isPair = row.pair !== undefined

  const markerClass = cn(
    'mt-[7px] h-2 w-2 shrink-0 rounded-full transition-colors duration-300',
    isActive && row.tone === 'gold' && 'bg-[var(--accent-gold)] shadow-[0_0_0_3px_var(--tint-gold-medium)]',
    isActive && row.tone === 'salmon' && 'bg-[var(--accent-salmon)] shadow-[0_0_0_3px_var(--tint-salmon-glow)]',
    isActive && (row.tone === 'neutral' || row.tone === 'done') && 'bg-foreground shadow-[0_0_0_3px_var(--tint-neutral-ring)]',
    !isActive && isPast && 'bg-text-faint',
    !isActive && !isPast && 'bg-border',
  )

  const textTone = cn(
    'transition-colors duration-300',
    isActive ? 'text-foreground' : isPast ? 'text-muted-foreground' : 'text-text-faint',
  )

  return (
    <li className="relative grid grid-cols-[auto_auto_1fr_auto] items-start gap-x-3 py-2 pl-3">
      {/* pair bracket on the far left — a gold bar spanning the two beat-3 rows */}
      {isPair && (
        <span
          aria-hidden
          className={cn(
            'absolute left-0 w-[2px] bg-[var(--accent-gold)] transition-opacity duration-300',
            row.pair === 'top' ? 'top-4 bottom-0' : 'top-0 bottom-4',
            isActive || isPast ? 'opacity-80' : 'opacity-30',
          )}
        />
      )}

      <span aria-hidden className={markerClass} />

      <span className={cn('font-mono text-[12px] tabular-nums', textTone)}>
        {row.lead}
      </span>
      <span className={cn('font-mono text-[12px]', textTone)}>
        {row.detail}
      </span>

      {/* "same tx" badge sits on the first of the pair rows only */}
      {row.pair === 'top' && (
        <span
          className={cn(
            'justify-self-end self-center rounded border border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)] px-1.5 py-0.5 font-mono text-[9px] uppercase tracking-[0.18em] transition-colors duration-300',
            isActive || isPast ? 'text-[var(--accent-gold)]' : 'text-text-faint opacity-60',
          )}
        >
          same tx
        </span>
      )}
    </li>
  )
}
