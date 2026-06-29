'use client'

// Payment-lifecycle scroll scene — the signature scroll choreography.
//
// A tall scroll track contains a sticky stage. As the user scrolls through the
// track, scroll progress maps to the five lifecycle beats:
//   discover → 402 quote → sign → settle (claim) → refund
// The right side reuses <EscrowDiagramAnimated> (the existing beat machine);
// the left side is a running editorial readout whose active beat crossfades.
//
// Reduced motion: the `.scene-pin` utility releases the sticky pin (becomes
// static) and `.scene-beat` forces every beat visible & stacked — the user
// reads the full sequence top-to-bottom with no scroll dependency, and the
// diagram sits on its resolved (beat 4) frame.

import { useEffect, useRef, useState } from 'react'
import { useReducedMotion } from 'motion/react'
import { cn } from '@/lib/utils'
import { EscrowDiagramAnimated, type EscrowBeat } from './escrow-diagram-animated'
import { ESCROW_PROGRAM_ID } from './config'

interface Beat {
  n: string
  key: EscrowBeat
  verb: string
  http: string
  body: string
  tone: 'neutral' | 'warn' | 'gold' | 'salmon' | 'done'
}

const BEATS: Beat[] = [
  {
    n: '01',
    key: 0,
    verb: 'discover',
    http: 'GET /.well-known/agent-card.json',
    body: 'The agent reads Solvela’s A2A AgentCard — capabilities, models, and the x402 payment terms it will be quoted against. No human, no dashboard.',
    tone: 'neutral',
  },
  {
    n: '02',
    key: 1,
    verb: 'get quoted',
    http: '402 payment required',
    body: 'It posts a request with no signature. The gateway answers 402 with a per-token USDC cost and a flat 5% platform fee — the exact amount, before a cent moves.',
    tone: 'warn',
  },
  {
    n: '03',
    key: 1,
    verb: 'sign + deposit',
    http: 'PAYMENT-SIGNATURE: ey…8fQ',
    body: 'The agent signs a USDC-SPL transaction that deposits the full quote into a trustless on-chain escrow. A wallet is the only credential — no accounts, no API keys.',
    tone: 'gold',
  },
  {
    n: '04',
    key: 3,
    verb: 'settle',
    http: '200 OK · escrow claim',
    body: 'The provider streams the response. In the same transaction, the escrow claims only what was delivered to the provider — never more than the work performed.',
    tone: 'done',
  },
  {
    n: '05',
    key: 4,
    verb: 'refund',
    http: 'refund → agent',
    body: 'Whatever the provider did not earn refunds to the agent on-chain, atomically with the claim. Pay only for what gets delivered — verifiably, not on trust.',
    tone: 'salmon',
  },
]

export function LifecycleScene() {
  const trackRef = useRef<HTMLDivElement>(null)
  const reduced = useReducedMotion()
  const [active, setActive] = useState(0)
  const [progress, setProgress] = useState(0)

  useEffect(() => {
    if (reduced) {
      setActive(BEATS.length - 1)
      setProgress(1)
      return
    }
    const node = trackRef.current
    if (!node) return

    let raf = 0
    const onScroll = () => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        const rect = node.getBoundingClientRect()
        const vh = window.innerHeight
        // Progress 0→1 across the portion of the track where the sticky
        // stage is fully pinned (track height minus one viewport).
        const total = rect.height - vh
        const scrolled = Math.min(Math.max(-rect.top, 0), total)
        const p = total > 0 ? scrolled / total : 0
        setProgress(p)
        // Even, full-width window per beat: each beat owns 1/N of the track.
        // `Math.floor(p * N)` divides the track into N equal slabs; the last
        // beat (refund) holds for its whole final slab — clamp guards p===1.
        const idx = Math.min(BEATS.length - 1, Math.floor(p * BEATS.length))
        setActive(idx)
      })
    }

    onScroll()
    window.addEventListener('scroll', onScroll, { passive: true })
    window.addEventListener('resize', onScroll)
    return () => {
      window.removeEventListener('scroll', onScroll)
      window.removeEventListener('resize', onScroll)
      if (raf) cancelAnimationFrame(raf)
    }
  }, [reduced])

  const beatForDiagram = BEATS[active]?.key ?? 0

  return (
    <section
      aria-labelledby="lifecycle-heading"
      className="relative border-t border-border/60 bg-[var(--popover)]"
    >
      {/* section header band */}
      <div className="mx-auto max-w-[1320px] px-6 pt-16 lg:pt-24">
        <div className="rule-tipped mb-7" />
        <div className="flex flex-col gap-3 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex flex-col gap-3">
            <span className="kicker">One transaction, start to finish</span>
            <h2
              id="lifecycle-heading"
              className="display-wonk max-w-[18ch] text-[var(--heading-color)]"
              style={{ fontSize: 'clamp(2rem, 4.6vw, 4rem)' }}
            >
              The whole payment, watched live.
            </h2>
          </div>
          <p className="max-w-[34ch] font-mono text-[11px] leading-[1.7] uppercase tracking-[0.14em] text-text-faint">
            scroll the column · five beats · escrow pda {ESCROW_PROGRAM_ID}…
          </p>
        </div>
      </div>

      {/* scroll track: tall enough for one viewport per beat. The sticky
          stage pins while the page scrolls through it. */}
      <div
        ref={trackRef}
        className="relative mt-10 lg:mt-14"
        style={{ height: reduced ? 'auto' : `${BEATS.length * 60}vh` }}
      >
        <div className="scene-pin flex flex-col justify-center">
          {/* scroll-progress affordance — a thin rule pinned to the top of the
              stage so a viewer who lands mid-section sees there's a track to
              traverse. Static (full) under reduced motion. */}
          {!reduced && (
            <div
              aria-hidden
              className="pointer-events-none absolute inset-x-0 top-0 h-px bg-border/50"
            >
              <div
                className="h-full origin-left bg-[var(--accent-salmon)] transition-[width] duration-150 ease-out"
                style={{ width: `${Math.round(progress * 100)}%` }}
              />
            </div>
          )}
          <div className="mx-auto grid w-full max-w-[1320px] items-center gap-8 px-6 py-10 lg:grid-cols-[minmax(0,0.46fr)_minmax(0,0.54fr)] lg:gap-14 lg:py-0">
            {/* left rail — beat copy (crossfades) + a static step index */}
            <div className="relative">
              {/* running progress index, brutalist hairline column */}
              <div className="mb-8 flex items-center gap-4 lg:mb-10">
                {BEATS.map((b, i) => (
                  <button
                    key={b.n}
                    type="button"
                    onClick={() => {
                      const node = trackRef.current
                      if (!node || reduced) return
                      const total = node.offsetHeight - window.innerHeight
                      const target =
                        node.offsetTop +
                        (total * (i + 0.5)) / BEATS.length
                      window.scrollTo({ top: target, behavior: 'smooth' })
                    }}
                    aria-label={`jump to step ${b.n}: ${b.verb}`}
                    aria-current={i === active ? 'step' : undefined}
                    className={cn(
                      'group flex flex-col items-start gap-1.5 outline-none',
                    )}
                  >
                    <span
                      className={cn(
                        'index-numeral text-[18px] tabular-nums transition-colors',
                        i === active
                          ? 'text-[var(--heading-color)]'
                          : i < active
                            ? 'text-muted-foreground'
                            : 'text-text-faint',
                      )}
                    >
                      {b.n}
                    </span>
                    <span
                      className={cn(
                        'h-[2px] w-7 transition-colors',
                        i === active
                          ? 'bg-[var(--accent-salmon)]'
                          : i < active
                            ? 'bg-muted-foreground'
                            : 'bg-border',
                      )}
                    />
                  </button>
                ))}
              </div>

              {/* stacked beats — only the active one is shown (crossfade).
                  Reduced motion forces all visible & stacked via CSS. */}
              <div className="relative min-h-[280px]">
                {BEATS.map((b, i) => (
                  <div
                    key={b.n}
                    data-active={i === active}
                    className={cn(
                      'scene-beat top-0 left-0 w-full',
                      reduced ? 'mb-12' : 'absolute',
                    )}
                  >
                    <div className="flex items-baseline gap-4">
                      <span className="index-numeral text-[clamp(3rem,7vw,5.5rem)] text-[var(--accent-salmon)]">
                        {b.n}
                      </span>
                      <h3
                        className="display-wonk text-[var(--heading-color)]"
                        style={{ fontSize: 'clamp(1.6rem, 3.4vw, 2.6rem)' }}
                      >
                        {b.verb}
                      </h3>
                    </div>

                    <span
                      className={cn(
                        'mt-4 inline-flex rounded border px-2.5 py-1 font-mono text-[11px] tracking-[0.06em]',
                        b.tone === 'warn' &&
                          'border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)] text-[var(--accent-salmon)]',
                        b.tone === 'gold' &&
                          'border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)] text-[var(--accent-gold)]',
                        b.tone === 'done' &&
                          'border-border bg-[var(--tint-neutral-ring)] text-foreground',
                        b.tone === 'salmon' &&
                          'border-[var(--accent-salmon)]/40 text-[var(--accent-salmon)]',
                        b.tone === 'neutral' &&
                          'border-border text-muted-foreground',
                      )}
                    >
                      {b.http}
                    </span>

                    <p className="mt-5 max-w-[44ch] text-[15px] leading-[1.65] text-muted-foreground">
                      {b.body}
                    </p>
                  </div>
                ))}
              </div>
            </div>

            {/* right — the live diagram, driven by the active beat */}
            <div className="relative">
              <div className="overflow-hidden rounded-lg border border-border bg-[var(--background)]">
                <div className="flex items-center justify-between border-b border-border/60 px-4 py-2.5 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint">
                  <span>escrow flow · usdc-spl</span>
                  <span className="text-[var(--accent-salmon)]">
                    beat {BEATS[active]?.n}
                  </span>
                </div>
                <div className="aspect-[640/340] w-full p-4 sm:p-6">
                  <EscrowDiagramAnimated beat={beatForDiagram} loopKey={active} />
                </div>
              </div>
              <p className="mt-3 pl-1 font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
                deposit · stream · claim + refund · same tx
              </p>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
