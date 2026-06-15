import Link from 'next/link'
import { Reveal } from '@/components/motion/primitives'
import { DOCS_URL, ESCROW_PROGRAM_ID } from './config'

/**
 * "Why agents pick solvela" — the four truth-verified claims, laid out as an
 * editorial spread rather than four identical cards.
 *
 * Layout: a wide featured claim (escrow, the trust anchor) spanning the top,
 * then three asymmetric claims below at varied widths. Each leads with an
 * oversized hairline index numeral and uses the claim-slab left-fill hover.
 */
export function WhySolvela() {
  return (
    <section
      aria-labelledby="why-heading"
      className="border-t border-border/60"
    >
      <div className="mx-auto max-w-[1320px] px-6 py-16 lg:py-24">
        <Reveal>
          <span className="eyebrow">why agents pick solvela</span>
          <h2
            id="why-heading"
            className="display-wonk mt-3 max-w-[16ch] text-[var(--heading-color)]"
            style={{ fontSize: 'clamp(2rem, 4.6vw, 4rem)' }}
          >
            Four guarantees, by construction.
          </h2>
        </Reveal>

        {/* featured claim — escrow, full width, the trust anchor */}
        <Reveal index={1} className="mt-12">
          <Link
            href={`${DOCS_URL}/docs/concepts/escrow`}
            className="claim-slab group block overflow-hidden rounded-lg border border-[var(--color-border-emphasis)] bg-[var(--card)] pl-7 transition-colors hover:bg-[var(--color-bg-surface-hover)]"
          >
            <div className="grid min-h-[260px] items-center gap-6 py-12 pr-7 lg:grid-cols-[auto_minmax(0,1fr)_auto] lg:gap-10 lg:py-14">
              <span className="index-numeral text-[clamp(4rem,9vw,7.5rem)] text-[var(--accent-gold)]">
                01
              </span>
              <div className="flex flex-col gap-3">
                <span className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint">
                  the trust anchor
                </span>
                <h3
                  className="display-wonk text-[var(--heading-color)]"
                  style={{ fontSize: 'clamp(1.6rem, 3.2vw, 2.6rem)' }}
                >
                  The only x402 gateway with trustless on-chain escrow.
                </h3>
                <p className="max-w-[60ch] text-[15px] leading-[1.6] text-muted-foreground">
                  Deposit, claim, and refund settle on-chain in one
                  transaction. The gateway can never claim more than the
                  provider delivered — the math is enforced by the program,
                  not by promise.
                </p>
                <span className="font-mono text-[11px] tracking-[0.04em] text-text-faint">
                  mainnet program {ESCROW_PROGRAM_ID}…
                </span>
              </div>
              <span className="hidden font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint opacity-0 transition-opacity group-hover:opacity-100 lg:inline">
                read docs →
              </span>
            </div>
          </Link>
        </Reveal>

        {/* three claims — asymmetric grid, not identical tiles */}
        <div className="mt-6 grid gap-6 lg:grid-cols-[minmax(0,1.25fr)_minmax(0,1fr)]">
          {/* cache — the wider one, paired with router below */}
          <div className="grid gap-6">
            <ClaimSlab
              n="02"
              kicker="exact-match cache"
              title="Zero false positives, by construction."
              body="The cache keys on the exact request. Identical prompts share one upstream call; anything different is a miss. There is no fuzzy match to be wrong — so it never serves you the wrong answer."
              mono="cache · exact match"
              href={`${DOCS_URL}/docs/concepts/cache`}
              accent="salmon"
            />
            <ClaimSlab
              n="03"
              kicker="deterministic router"
              title="Fifteen dimensions, zero external calls."
              body="A rule-based router scores every request across 15 dimensions and picks a model — entirely in-process. No classifier API, no extra round-trip, no surprise dependency on someone else’s uptime."
              mono="router · 0 external calls"
              href={`${DOCS_URL}/docs/concepts/smart-router`}
              accent="gold"
            />
          </div>

          {/* a2a — the tall companion, full-height emphasis */}
          <ClaimSlab
            n="04"
            kicker="a2a, end to end"
            title="Agents discover, negotiate, and pay — no human."
            body="Solvela publishes a live A2A AgentCard. Another agent can fetch it, read the x402 terms, and complete a paid call from discovery to settlement without a person ever touching the loop."
            mono="GET /.well-known/agent-card.json"
            href={`${DOCS_URL}/docs/api`}
            accent="salmon"
            tall
          />
        </div>
      </div>
    </section>
  )
}

interface ClaimSlabProps {
  n: string
  kicker: string
  title: string
  body: string
  mono: string
  href: string
  accent: 'salmon' | 'gold'
  tall?: boolean
}

function ClaimSlab({
  n,
  kicker,
  title,
  body,
  mono,
  href,
  accent,
  tall,
}: ClaimSlabProps) {
  const numColor =
    accent === 'gold' ? 'text-[var(--accent-gold)]' : 'text-[var(--accent-salmon)]'
  return (
    <Reveal index={2} className="h-full">
      <Link
        href={href}
        className={
          'claim-slab group flex h-full flex-col justify-between gap-8 overflow-hidden rounded-lg border border-border bg-[var(--card)] pl-7 transition-colors hover:bg-[var(--color-bg-surface-hover)]' +
          (tall ? ' min-h-[420px] py-12 pr-7' : ' min-h-[300px] py-10 pr-7')
        }
      >
        <div className="flex flex-col gap-3">
          <div className="flex items-start justify-between gap-4">
            <span className={`index-numeral text-[clamp(2.5rem,5vw,4rem)] ${numColor}`}>
              {n}
            </span>
            <span className="mt-2 font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint">
              {kicker}
            </span>
          </div>
          <h3
            className="display-wonk text-[var(--heading-color)]"
            style={{ fontSize: tall ? 'clamp(1.5rem, 2.8vw, 2.1rem)' : 'clamp(1.25rem, 2.2vw, 1.7rem)' }}
          >
            {title}
          </h3>
          <p className={`text-[15px] leading-[1.7] text-muted-foreground ${tall ? 'max-w-[42ch]' : 'max-w-[48ch]'}`}>
            {body}
          </p>
        </div>
        <div className="flex items-center justify-between gap-3">
          <span className="inline-flex items-center rounded border border-border/60 px-2 py-1 font-mono text-[11px] text-text-faint">
            {mono}
          </span>
          <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint opacity-0 transition-opacity group-hover:opacity-100">
            read docs →
          </span>
        </div>
      </Link>
    </Reveal>
  )
}
