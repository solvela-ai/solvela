import { Reveal } from '@/components/motion/primitives'
import { PROVIDERS } from './config'

/**
 * Providers — an editorial poster, not a tile grid. A poster-scale figure ("25
 * paid models") anchors the left; the five marquee providers run as a measured
 * list down the right, each a hairline row with its brand dot. The free NVIDIA
 * NIM tier is noted in the mono footnote below, not counted in the poster figure.
 */
export function ProviderRow() {
  return (
    <section
      aria-labelledby="providers-heading"
      className="border-t border-border/60 bg-[var(--popover)]"
    >
      <div className="mx-auto max-w-[1320px] px-6 py-16 lg:py-24">
        <div className="rule-tipped mb-10" />
        <div className="grid gap-12 lg:grid-cols-[minmax(0,0.95fr)_minmax(0,1.05fr)] lg:gap-16">
          {/* left — the poster figure */}
          <Reveal className="flex flex-col justify-center">
            {/* ponytail: no eyebrow — the poster "25 paid models" figure self-labels */}
            <div className="flex items-end gap-4">
              <span className="poster-figure text-[clamp(6.5rem,18vw,14rem)]">
                25
              </span>
              <div className="mb-3 flex flex-col">
                <span className="display-calm text-[clamp(1.4rem,3vw,2.4rem)] font-normal text-muted-foreground">
                  paid models
                </span>
                <span className="font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint">
                  five providers
                </span>
              </div>
            </div>
            <h2
              id="providers-heading"
              className="display-wonk mt-8 max-w-[20ch] text-[var(--heading-color)]"
              style={{ fontSize: 'clamp(1.5rem, 3vw, 2.5rem)' }}
            >
              One endpoint. One USDC price book. Five frontier labs.
            </h2>
            <p className="mt-5 max-w-[44ch] font-mono text-[11px] uppercase tracking-[0.14em] leading-[1.8] text-text-faint">
              call model=&quot;auto&quot; and the router places the request —
              or name a model directly. unified per-token pricing in usdc.
              plus a free nvidia nim tier — 15 zero-cost nemotron models.
            </p>
          </Reveal>

          {/* right — measured provider list */}
          <Reveal index={1} className="flex flex-col justify-center">
            <ul className="flex list-none flex-col" role="list">
              {PROVIDERS.map((p, i) => (
                <li key={p.id}>
                  <div
                    className={
                      'group flex items-center justify-between gap-4 py-7 transition-colors hover:text-foreground' +
                      (i === 0 ? ' border-t border-border/60' : '') +
                      ' border-b border-border/60'
                    }
                  >
                    <div className="flex items-center gap-5">
                      <span className="index-numeral w-10 text-[26px] text-text-faint transition-colors group-hover:text-muted-foreground">
                        {String(i + 1).padStart(2, '0')}
                      </span>
                      <span className="flex items-center gap-3">
                        <span
                          aria-hidden
                          className="inline-block h-2.5 w-2.5 rounded-full transition-transform group-hover:scale-125"
                          style={{ backgroundColor: p.dot }}
                        />
                        <span className="display-calm text-[clamp(1.25rem,2.4vw,1.75rem)] font-normal text-foreground">
                          {p.name}
                        </span>
                      </span>
                    </div>
                    <span className="font-mono text-[10px] uppercase tracking-[0.2em] text-[var(--color-success)]">
                      live
                    </span>
                  </div>
                </li>
              ))}
            </ul>
          </Reveal>
        </div>
      </div>
    </section>
  )
}
