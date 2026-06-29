import { Reveal } from '@/components/motion/primitives'
import { ArchitectureDiagram } from './architecture-diagram'
import { DOCS_URL } from './config'

/**
 * "Under the hood" — the static reference topology: SDKs and A2A agents in,
 * gateway internals (x402 verify, router, cache, guard), five providers out,
 * Solana settlement plane underneath. Both stores labeled optional because
 * the gateway genuinely degrades gracefully without them.
 */
export function ArchitectureSection() {
  return (
    <section aria-labelledby="arch-heading" className="border-t border-border/60">
      <div className="mx-auto max-w-[1320px] px-6 py-16 lg:py-24">
        <Reveal>
          <span className="kicker">Under the hood</span>
          <h2
            id="arch-heading"
            className="display-wonk mt-3 max-w-[18ch] text-[var(--heading-color)]"
            style={{ fontSize: 'clamp(2rem, 4.6vw, 4rem)' }}
          >
            One gateway. Two ways to settle.
          </h2>
        </Reveal>
        <Reveal index={1} className="mt-10">
          <div className="hidden md:block">
            <ArchitectureDiagram />
          </div>
          <div className="md:hidden">
            <ArchitectureDiagram compact />
          </div>
        </Reveal>
        <Reveal index={2}>
          <p className="mt-6 max-w-[68ch] text-[14px] leading-[1.6] text-muted-foreground">
            Exact payments verify up front and settle after delivery; escrow
            deposits land on-chain first, then claim and refund settle in one
            transaction. Redis and Postgres are optional — the gateway degrades
            gracefully without either.{' '}
            <a
              href={`${DOCS_URL}/docs/concepts/architecture`}
              className="font-mono text-[12px] uppercase tracking-[0.14em] text-[var(--accent-salmon)] hover:text-[var(--accent-salmon-hover)]"
            >
              architecture docs →
            </a>
          </p>
        </Reveal>
      </div>
    </section>
  )
}
