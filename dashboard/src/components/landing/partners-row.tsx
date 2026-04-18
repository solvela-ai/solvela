// Lightweight partners / built-on row — text-mark treatments, no raster logos.
// Replace with real SVG marks as they become available.

const PARTNERS = [
  { id: 'solana', text: 'solana', mark: '◎' },
  { id: 'telsi', text: 'telsi.ai', mark: '✦' },
  { id: 'rustyclaw', text: 'rusty claw', mark: '◇' },
  { id: 'circle', text: 'circle · usdc', mark: '◐' },
  { id: 'x402', text: 'x402 · linux foundation', mark: '▲' },
]

export function PartnersRow() {
  return (
    <section className="border-t border-border/60 bg-[var(--background)]">
      <div className="mx-auto flex max-w-[1280px] flex-col gap-5 px-6 py-10 sm:flex-row sm:items-center sm:gap-10">
        <span className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint whitespace-nowrap">
          built on · powering · partners
        </span>
        <div className="flex flex-wrap items-center gap-x-8 gap-y-4 sm:gap-x-10">
          {PARTNERS.map((p) => (
            <div
              key={p.id}
              className="group flex items-center gap-2.5 text-muted-foreground transition-colors hover:text-foreground"
            >
              <span
                aria-hidden
                className="font-display text-[18px] leading-none text-[var(--accent-salmon)]/80 transition-colors group-hover:text-[var(--accent-salmon)]"
              >
                {p.mark}
              </span>
              <span className="font-display text-[16px] font-semibold tracking-[-0.01em]">
                {p.text}
              </span>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
