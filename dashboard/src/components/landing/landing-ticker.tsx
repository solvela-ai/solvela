'use client'

import { useEffect, useRef, useState } from 'react'

const TICKER_ITEMS = [
  'ESCROW · 9neDHouXgEgHZDde5Sp',
  'p50 · 38ms',
  'p99 · 184ms',
  '26 models · 5 providers',
  '5% flat fee',
  'mainnet · x402 · usdc-spl',
  'a2a · agent-card · /.well-known',
]

export function LandingTicker() {
  const track = [...TICKER_ITEMS, ...TICKER_ITEMS, ...TICKER_ITEMS]
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [inView, setInView] = useState(true)

  useEffect(() => {
    const node = wrapperRef.current
    if (!node) return
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) setInView(e.isIntersecting)
      },
      { rootMargin: '100px 0px' }
    )
    io.observe(node)
    return () => io.disconnect()
  }, [])

  return (
    <div
      ref={wrapperRef}
      className="relative z-10 overflow-hidden border-y border-border/60 bg-[var(--popover)]"
      aria-label="platform metrics ticker"
    >
      <div
        className="ticker-track py-2 font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint"
        style={{ animationPlayState: inView ? 'running' : 'paused' }}
      >
        {track.map((t, i) => (
          <span
            key={i}
            className="inline-flex items-center gap-3"
            aria-hidden={i >= TICKER_ITEMS.length}
          >
            <span className="inline-block h-1 w-1 rounded-full bg-[var(--accent-salmon)]" />
            {t}
          </span>
        ))}
      </div>
    </div>
  )
}
