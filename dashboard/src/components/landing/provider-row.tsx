import { PROVIDERS } from './config'

export function ProviderRow() {
  return (
    <section aria-labelledby="providers-heading" className="border-t border-border/60">
      <div className="mx-auto max-w-[1280px] px-6 py-14">
        <div className="flex flex-col gap-6 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex flex-col gap-2">
            <span className="eyebrow">providers</span>
            <h2
              id="providers-heading"
              className="font-display leading-[1.05] text-foreground"
              style={{ fontSize: 'clamp(1.5rem, 2.4vw, 2rem)', fontWeight: 600 }}
            >
              One endpoint. Five providers. Twenty-six models.
            </h2>
          </div>
          <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint">
            unified usdc pricing · model=&quot;auto&quot;
          </span>
        </div>

        <div className="mt-8 grid grid-cols-2 gap-3 sm:grid-cols-5">
          {PROVIDERS.map((p) => (
            <div
              key={p.id}
              className="provider-pill flex items-center justify-between rounded-md border border-border bg-[var(--card)] px-4 py-3 font-mono text-[13px] text-foreground"
            >
              <div className="flex items-center gap-2.5">
                <span
                  aria-hidden
                  className="h-2 w-2 rounded-full"
                  style={{ backgroundColor: p.dot }}
                />
                <span>{p.name}</span>
              </div>
              <span className="text-[10px] uppercase tracking-[0.18em] text-text-faint">
                live
              </span>
            </div>
          ))}
        </div>
      </div>
    </section>
  )
}
