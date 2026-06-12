'use client'

import { useEffect, useRef, useState } from 'react'

/**
 * Settlement readout-strip — a thin, lowercase mono ticker of plausible-but-
 * clearly-illustrative settlement events. Uses the `.readout-strip` utility
 * (masked edges, marquee paused on hover, never runs under reduced motion).
 *
 * These lines are ILLUSTRATIVE texture, not live data — the `sample` tag and
 * lowercase voice signal that. No latency/throughput claims appear here.
 */
const EVENTS = [
  'deposit 0.0042 usdc → escrow 9neDHouX',
  'claim 0.0038 → provider · refund 0.0004 → agent · same tx',
  '402 quote · model auto · fee 0.0002 (5%)',
  'agent-card fetched · /.well-known/agent.json',
  'cache hit · exact match · upstream skipped',
  'router → tier complex · 15-dim · 0 external calls',
  'deposit 0.0117 usdc → escrow · nonce 4c1d',
  'settled · usdc-spl · mainnet',
  'free-tier call · zero-cost model · rate-limited',
  'a2a negotiate → settle · no human in loop',
] as const

export function SettlementReadout() {
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [inView, setInView] = useState(true)

  useEffect(() => {
    const node = wrapperRef.current
    if (!node) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setInView(e.isIntersecting)
      },
      { rootMargin: '120px 0px' },
    )
    io.observe(node)
    return () => io.disconnect()
  }, [])

  // Two copies of the sequence make the -50% marquee loop seamless.
  const track = [...EVENTS, ...EVENTS]

  return (
    <div
      ref={wrapperRef}
      className="readout-strip relative w-full text-text-faint"
      aria-label="illustrative settlement events"
    >
      <div
        className="readout-strip__track"
        style={{ animationPlayState: inView ? 'running' : 'paused' }}
      >
        {track.map((e, i) => (
          <span
            key={i}
            className="inline-flex items-center gap-2.5"
            aria-hidden={i >= EVENTS.length}
          >
            <span
              aria-hidden
              className="inline-block h-1 w-1 rounded-full bg-[var(--accent-gold)]"
            />
            <span className="tabular-nums">{e}</span>
          </span>
        ))}
      </div>
    </div>
  )
}
