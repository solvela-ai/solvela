'use client'

import { useEffect, useRef, useState } from 'react'
import { cn } from '@/lib/utils'

type Side = 'agent' | 'gateway'

interface Line {
  side: Side
  label: string
  detail: string
  tone?: 'default' | 'warn' | 'ok'
}

const LINES: Line[] = [
  {
    side: 'agent',
    label: 'POST /v1/chat/completions',
    detail: 'model: auto  ·  messages: [...]',
  },
  {
    side: 'gateway',
    label: '402 payment required',
    detail: 'cost 0.0042 usdc  ·  fee 0.0002 (5%)',
    tone: 'warn',
  },
  {
    side: 'agent',
    label: 'PAYMENT-SIGNATURE: ey…8fQ',
    detail: 'escrow deposit signed  ·  nonce 91a4',
  },
  {
    side: 'gateway',
    label: '200 OK  ·  streaming',
    detail: '"Hello — I can help with that."',
    tone: 'ok',
  },
]

const TOTAL_STEPS = LINES.length + 1 // +1 for the escrow claim row
const INTERVAL_MS = 1150
const HOLD_MS = 1800

export function HeroHandshake() {
  const [step, setStep] = useState(0)
  const wrapperRef = useRef<HTMLDivElement>(null)
  const [inView, setInView] = useState(true)

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

  useEffect(() => {
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches

    let mounted = true
    let timer: ReturnType<typeof setTimeout>

    if (reduced) {
      requestAnimationFrame(() => mounted && setStep(TOTAL_STEPS))
      return () => {
        mounted = false
      }
    }

    if (!inView) {
      // Pause loop when scrolled off-viewport; do not reset step so it resumes smoothly.
      return () => {
        mounted = false
      }
    }

    const schedule = (value: number) => {
      if (!mounted) return
      setStep(value)
      const nextValue = value >= TOTAL_STEPS ? 0 : value + 1
      const delay =
        value >= TOTAL_STEPS
          ? HOLD_MS
          : value === TOTAL_STEPS - 1
          ? HOLD_MS
          : INTERVAL_MS
      timer = setTimeout(() => schedule(nextValue), delay)
    }

    timer = setTimeout(() => schedule(step === 0 ? 1 : step), 600)

    return () => {
      mounted = false
      clearTimeout(timer)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [inView])

  const claimActive = step > LINES.length
  const lastActive = LINES[Math.max(0, Math.min(LINES.length - 1, step - 1))]
  const agentActive = step > 0 && step <= LINES.length && lastActive?.side === 'agent'
  const gatewayActive = step > 0 && step <= LINES.length && lastActive?.side === 'gateway'

  return (
    <div ref={wrapperRef} className="terminal-card select-none">
      <div className="terminal-card-titlebar">
        <span className="terminal-card-dots" aria-hidden>
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot terminal-card-dot--accent" />
        </span>
        <span>402 handshake · mainnet</span>
        <span className="ml-auto text-[10px] tracking-[0.18em] text-text-faint">
          live
        </span>
      </div>

      <div className="terminal-card-screen !px-0 !py-0">
        {/* column headers */}
        <div className="grid grid-cols-2 border-b border-border/60 text-[10px] font-mono uppercase tracking-[0.2em] text-text-faint">
          <div className="flex items-center gap-2 px-5 py-3">
            <span
              className={cn(
                'h-1.5 w-1.5 rounded-full bg-foreground/70 transition-[box-shadow,background-color] duration-300',
                agentActive &&
                  'bg-foreground shadow-[0_0_0_3px_var(--tint-neutral-ring)]'
              )}
            />
            agent
          </div>
          <div className="flex items-center gap-2 border-l border-border/60 px-5 py-3">
            <span
              className={cn(
                'h-1.5 w-1.5 rounded-full bg-[var(--accent-salmon)] transition-[box-shadow,background-color] duration-300',
                gatewayActive &&
                  'shadow-[0_0_0_3px_var(--tint-salmon-glow)]'
              )}
            />
            gateway · solvela.ai
          </div>
        </div>

        {/* packet rows */}
        <div className="divide-y divide-border/50">
          {LINES.map((line, i) => {
            const visible = step > i
            const isAgent = line.side === 'agent'
            return (
              <div
                key={i}
                className="grid grid-cols-2"
                aria-hidden={!visible}
              >
                {/* agent column */}
                <div
                  className={cn(
                    'flex flex-col gap-1 px-5 py-4 min-h-[64px] transition-opacity duration-[420ms]',
                    isAgent ? 'items-start' : 'items-start',
                    visible && isAgent ? 'opacity-100' : 'opacity-0'
                  )}
                >
                  {isAgent && visible && (
                    <>
                      <div className="flex items-center gap-2 font-mono text-[13px] text-foreground">
                        <span className="text-[var(--accent-salmon)]">→</span>
                        <span>{line.label}</span>
                      </div>
                      <div className="pl-4 font-mono text-[11px] text-muted-foreground">
                        {line.detail}
                      </div>
                    </>
                  )}
                </div>

                {/* gateway column */}
                <div
                  className={cn(
                    'flex flex-col gap-1 border-l border-border/60 px-5 py-4 min-h-[64px] transition-opacity duration-[420ms]',
                    visible && !isAgent ? 'opacity-100' : 'opacity-0'
                  )}
                >
                  {!isAgent && visible && (
                    <>
                      <div className="flex items-center gap-2 font-mono text-[13px]">
                        <span
                          className={cn(
                            line.tone === 'warn' && 'text-[var(--callout-warn)]',
                            line.tone === 'ok' && 'text-[var(--color-success)]',
                            (!line.tone || line.tone === 'default') && 'text-foreground',
                          )}
                        >
                          ←
                        </span>
                        <span
                          className={cn(
                            line.tone === 'warn' && 'text-[var(--callout-warn)]',
                            line.tone === 'ok' && 'text-[var(--color-success)]',
                            (!line.tone || line.tone === 'default') && 'text-foreground',
                          )}
                        >
                          {line.label}
                        </span>
                      </div>
                      <div className="pl-4 font-mono text-[11px] text-muted-foreground">
                        {line.detail}
                      </div>
                    </>
                  )}
                </div>
              </div>
            )
          })}
        </div>

        {/* escrow claim row (full width) */}
        <div
          className={cn(
            'flex items-center justify-between border-t border-border/60 px-5 py-4 transition-[background-color,border-color,opacity] duration-[560ms]',
            claimActive
              ? 'opacity-100 border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)]'
              : 'opacity-60'
          )}
        >
          <div className="flex items-center gap-3">
            <span
              className={cn(
                'h-2 w-2 rounded-full transition-colors',
                claimActive
                  ? 'bg-[var(--color-border-emphasis)] shadow-[0_0_0_4px_var(--tint-gold-medium)]'
                  : 'bg-border'
              )}
            />
            <span className="font-mono text-[11px] uppercase tracking-[0.2em] text-text-faint">
              escrow
            </span>
            <span className="font-mono text-[11px] text-muted-foreground">
              pda · {ESCROW_TEASE}
            </span>
          </div>
          <span
            className={cn(
              'font-mono text-[11px] uppercase tracking-[0.18em] transition-colors',
              claimActive ? 'text-foreground' : 'text-text-faint'
            )}
          >
            {claimActive ? 'claimed ✓' : 'pending'}
          </span>
        </div>
      </div>
    </div>
  )
}

const ESCROW_TEASE = '9neDHouXgEgHZDde5Sp…'
