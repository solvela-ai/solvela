import { EscrowDiagram } from './escrow-diagram'
import { InView } from './in-view'

export function EscrowPanel() {
  return (
    <section className="relative border-t border-border/60 bg-[var(--popover)]">
      <div className="mx-auto max-w-[1280px]">
        <div className="grid gap-0 lg:grid-cols-[minmax(0,0.4fr)_minmax(0,0.6fr)]">
          {/* left rail — pinned copy */}
          <div className="flex flex-col gap-6 border-b border-border/60 px-6 py-16 lg:border-b-0 lg:border-r lg:py-28">
            <span className="eyebrow">the diamond</span>
            <h2
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

            <ul className="mt-2 flex flex-col gap-4 text-[15px] leading-[1.55] text-muted-foreground">
              <li className="flex gap-3">
                <span
                  aria-hidden
                  className="mt-[9px] h-[6px] w-[6px] flex-shrink-0 rounded-full bg-[var(--color-border-emphasis)]"
                />
                <span>
                  <strong className="text-foreground">Deposit</strong> to an
                  on-chain PDA at request time. No upfront full payment.
                </span>
              </li>
              <li className="flex gap-3">
                <span
                  aria-hidden
                  className="mt-[9px] h-[6px] w-[6px] flex-shrink-0 rounded-full bg-[var(--color-border-emphasis)]"
                />
                <span>
                  <strong className="text-foreground">Claim</strong> fires only
                  after the provider streams a real response. No-response → no
                  claim.
                </span>
              </li>
              <li className="flex gap-3">
                <span
                  aria-hidden
                  className="mt-[9px] h-[6px] w-[6px] flex-shrink-0 rounded-full bg-[var(--color-border-emphasis)]"
                />
                <span>
                  <strong className="text-foreground">Refund</strong> returns
                  unclaimed funds to the agent wallet in the same tx.
                </span>
              </li>
            </ul>

            <div className="mt-4 flex flex-wrap items-center gap-3 font-mono text-[11px] uppercase tracking-[0.16em] text-text-faint">
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

            <p className="mt-4 max-w-[44ch] font-mono text-[11px] leading-[1.6] text-text-faint">
              No other x402 LLM gateway has trustless on-chain escrow.
            </p>
          </div>

          {/* right — isometric diagram, bleeds to right edge */}
          <div className="relative min-h-[420px] overflow-hidden bg-[var(--background)] bg-grid-dense">
            <div className="absolute inset-0 flex items-center justify-center p-6 lg:p-10">
              <InView className="h-full w-full max-w-[780px]">
                <EscrowDiagram />
              </InView>
            </div>
          </div>
        </div>
      </div>
    </section>
  )
}
