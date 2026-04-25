'use client'

import { useRef, useState, type KeyboardEvent } from 'react'
import { ArrowRight } from 'lucide-react'
import { cn } from '@/lib/utils'
import { CopyButton } from './copy-button'
import { CURL_SNIPPET, QUICKSTART_URL } from './config'
import { SAMPLES, WORKS_WITH, type SampleStatus } from './sdk-samples'

interface SdkCtaPanelProps {
  /**
   * Pre-rendered Shiki HTML per sample id (inner <code>…</code> markup,
   * no outer <pre>). Produced on the server so the highlighter never
   * ships to the client bundle.
   */
  preHighlighted: Record<string, string>
}

function StatusDot({ status }: { status: SampleStatus }) {
  const color =
    status === 'live'
      ? 'bg-[var(--color-success)]'
      : status === 'alpha'
      ? 'bg-[var(--accent-gold)]'
      : 'bg-text-faint'
  return (
    <span
      aria-label={status}
      className={`inline-block h-1.5 w-1.5 rounded-full ${color}`}
    />
  )
}

export function SdkCtaPanel({ preHighlighted }: SdkCtaPanelProps) {
  const [activeId, setActiveId] = useState<string>('ts')
  const active = SAMPLES.find((s) => s.id === activeId) ?? SAMPLES[0]
  const activeHtml = preHighlighted[active.id]
  const tabRefs = useRef<Record<string, HTMLButtonElement | null>>({})

  function focusTab(id: string) {
    setActiveId(id)
    // Wait one tick for React to render then move focus to the new tab.
    requestAnimationFrame(() => tabRefs.current[id]?.focus())
  }

  function handleTabKey(e: KeyboardEvent<HTMLButtonElement>, idx: number) {
    const last = SAMPLES.length - 1
    let nextIdx: number | null = null
    if (e.key === 'ArrowRight') nextIdx = idx === last ? 0 : idx + 1
    else if (e.key === 'ArrowLeft') nextIdx = idx === 0 ? last : idx - 1
    else if (e.key === 'Home') nextIdx = 0
    else if (e.key === 'End') nextIdx = last
    if (nextIdx !== null) {
      e.preventDefault()
      focusTab(SAMPLES[nextIdx].id)
    }
  }

  return (
    <section
      aria-labelledby="sdk-cta-heading"
      className="border-t border-border/60 bg-[var(--popover)]"
    >
      <div className="mx-auto max-w-[1280px] px-6 py-16 lg:py-24">
        <div className="flex flex-col gap-10">
          {/* headline row */}
          <div className="flex flex-col gap-3">
            <span className="eyebrow">your first 402</span>
            <h2
              id="sdk-cta-heading"
              className="font-display leading-[1.0] tracking-[-0.02em] text-foreground"
              style={{ fontSize: 'clamp(2rem, 4vw, 3.25rem)', fontWeight: 600 }}
            >
              A wallet is your API key.
              <br />
              <span className="text-muted-foreground">
                Sign, send, receive. That&apos;s it.
              </span>
            </h2>
          </div>

          {/* works-with strip */}
          <div className="flex flex-wrap items-center gap-x-6 gap-y-3 font-mono text-[11px] uppercase tracking-[0.18em] text-muted-foreground">
            <span className="text-text-faint">works with</span>
            {WORKS_WITH.map((w) => (
              <span
                key={w.label}
                className="inline-flex items-center gap-2"
                title={w.status}
              >
                <StatusDot status={w.status} />
                <span>{w.label}</span>
                {w.status !== 'live' && (
                  <span className="text-text-faint">· {w.status}</span>
                )}
              </span>
            ))}
          </div>

          {/* big terminal card */}
          <div className="terminal-card">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots" aria-hidden>
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot terminal-card-dot--accent" />
              </span>
              <span>solvela · quickstart</span>
              <span className="ml-auto hidden font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint sm:inline">
                {active.install}
              </span>
            </div>

            {/* tabs */}
            <div
              role="tablist"
              aria-label="SDK quickstart samples"
              className="flex overflow-x-auto border-b border-border/60"
            >
              {SAMPLES.map((s, idx) => {
                const selected = s.id === activeId
                return (
                  <button
                    key={s.id}
                    ref={(el) => {
                      tabRefs.current[s.id] = el
                    }}
                    type="button"
                    role="tab"
                    id={`tab-${s.id}`}
                    aria-selected={selected}
                    aria-controls={`tabpanel-${s.id}`}
                    tabIndex={selected ? 0 : -1}
                    onClick={() => setActiveId(s.id)}
                    onKeyDown={(e) => handleTabKey(e, idx)}
                    className={cn(
                      'relative inline-flex min-h-11 items-center gap-2 px-4 py-3 font-mono text-[11px] uppercase tracking-[0.18em] transition-colors',
                      selected
                        ? 'text-foreground'
                        : 'text-text-faint hover:text-muted-foreground',
                    )}
                  >
                    {s.status && <StatusDot status={s.status} />}
                    <span>{s.label}</span>
                    {selected && (
                      <span
                        aria-hidden
                        className="absolute bottom-0 left-3 right-3 h-[2px] bg-[var(--accent-salmon)]"
                      />
                    )}
                  </button>
                )
              })}
            </div>

            {/* code body */}
            <div className="terminal-card-screen !p-0">
              <div className="flex items-center justify-between border-b border-border/60 px-5 py-2 font-mono text-[11px] text-text-faint">
                <div className="flex items-center gap-3">
                  <span>{active.install}</span>
                  {active.status && active.status !== 'live' && (
                    <span className="rounded border border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)] px-1.5 py-0.5 text-[10px] uppercase tracking-[0.18em] text-[var(--accent-gold)]">
                      {active.status} — not yet published
                    </span>
                  )}
                </div>
                <CopyButton text={active.code} label="copy" />
              </div>
              {activeHtml ? (
                <pre
                  key={active.id}
                  role="tabpanel"
                  id={`tabpanel-${active.id}`}
                  aria-labelledby={`tab-${active.id}`}
                  tabIndex={0}
                  className="m-0 animate-fade-in overflow-x-auto border-0 !bg-transparent px-5 py-6 font-mono text-[13px] leading-[1.7] text-foreground"
                  dangerouslySetInnerHTML={{ __html: activeHtml }}
                />
              ) : (
                <pre
                  key={active.id}
                  role="tabpanel"
                  id={`tabpanel-${active.id}`}
                  aria-labelledby={`tab-${active.id}`}
                  tabIndex={0}
                  className="m-0 animate-fade-in overflow-x-auto border-0 !bg-transparent px-5 py-6 font-mono text-[13px] leading-[1.7] text-foreground"
                >
                  <code>{active.code}</code>
                </pre>
              )}
            </div>

            {/* footer row — curl + CTA */}
            <div className="flex flex-col gap-4 border-t border-border/60 px-5 py-5 sm:flex-row sm:items-center sm:justify-between">
              <div className="min-w-0 flex-1">
                <div className="mb-1 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint">
                  or, raw:
                </div>
                <div className="flex items-center gap-3">
                  <pre className="m-0 flex-1 overflow-x-auto border-0 !bg-transparent p-0 font-mono text-[12px]">
                    <code>
                      <span className="text-[var(--accent-salmon)]">curl</span>{' '}
                      <span className="text-[var(--accent-gold)]">
                        https://api.solvela.ai/v1/chat/completions
                      </span>{' '}
                      <span className="text-text-faint">…</span>
                    </code>
                  </pre>
                  <CopyButton text={CURL_SNIPPET} label="copy curl" />
                </div>
              </div>
              <a
                href={QUICKSTART_URL}
                className="inline-flex items-center gap-2 self-start rounded-md bg-[var(--accent-salmon)] px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-[var(--color-bg-inset)] shadow-[0_0_0_0_transparent] transition-all duration-200 hover:-translate-y-[1px] hover:bg-[var(--accent-salmon-hover)] hover:shadow-[0_8px_24px_-10px_var(--tint-salmon-drop)] active:translate-y-0 sm:self-auto"
              >
                open quickstart
                <ArrowRight className="h-3.5 w-3.5" />
              </a>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
