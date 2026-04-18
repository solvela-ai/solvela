import Link from 'next/link'
import { ArrowRight } from 'lucide-react'
import { HeroHandshake } from './hero-handshake'
import { InlineMetrics } from './inline-metrics'
import { QUICKSTART_URL, DOCS_URL } from './config'

export function HeroPanel() {
  return (
    <section className="landing-hero-bg relative overflow-hidden">
      {/* subtle radial soft light */}
      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 opacity-60"
        style={{
          background:
            'radial-gradient(circle at 85% 10%, rgba(254,129,129,0.05), transparent 45%)',
        }}
      />

      <div className="relative mx-auto max-w-[1280px] px-6 pb-16 pt-20 sm:pt-24 lg:pb-24 lg:pt-32">
        <div className="grid gap-14 lg:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)] lg:gap-16">
          {/* left — copy */}
          <div className="flex flex-col gap-8 animate-fade-in-up">
            <div className="flex flex-wrap items-center gap-3">
              <span className="eyebrow">Escrow-settled payments for agents</span>
              <span className="inline-flex items-center gap-1.5 rounded-full border border-[var(--color-border-emphasis)] bg-[rgba(200,162,64,0.08)] px-2.5 py-1 font-mono text-[10px] uppercase tracking-[0.16em] text-[#e0c27a]">
                <span
                  aria-hidden
                  className="inline-block h-1.5 w-1.5 rounded-full bg-[#e0c27a]"
                />
                only x402 gateway with on-chain escrow
              </span>
            </div>

            <h1
              className="font-display text-foreground leading-[0.98] tracking-[-0.03em]"
              style={{
                fontSize: 'clamp(3rem, 7vw, 6rem)',
                fontWeight: 600,
              }}
            >
              <span
                className="block font-black -ml-[2px]"
                style={{ fontWeight: 900 }}
              >
                Trustless
              </span>
              <span className="block">escrow for</span>
              <span className="block">agent payments.</span>
            </h1>

            <p className="max-w-[56ch] text-[17px] leading-[1.55] text-muted-foreground sm:text-[18px]">
              Your agent pays per call in USDC on Solana. The gateway only
              claims what the provider actually delivered — refund the rest
              on-chain, in the same transaction.
            </p>

            <div className="flex flex-wrap items-center gap-3">
              <a
                href={QUICKSTART_URL}
                className="inline-flex items-center gap-2 rounded-md bg-foreground px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-[var(--primary-foreground)] transition-colors hover:bg-[var(--heading-color)]"
              >
                start building
                <ArrowRight className="h-3.5 w-3.5" />
              </a>
              <Link
                href={DOCS_URL}
                className="inline-flex items-center gap-2 rounded-md border border-border px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-foreground transition-colors hover:border-[var(--accent-salmon)] hover:text-[var(--accent-salmon)]"
              >
                read docs
              </Link>
              <span className="hidden md:inline font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint ml-2">
                · no accounts · no api keys · just wallets
              </span>
            </div>

            <div className="pt-4">
              <InlineMetrics />
            </div>
          </div>

          {/* right — bilateral handshake */}
          <div className="animate-fade-in-up delay-2 lg:pt-6">
            <HeroHandshake />
            <p className="mt-3 pl-1 font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
              live 402 handshake · agent ⇄ gateway ⇄ escrow
            </p>
          </div>
        </div>
      </div>
    </section>
  )
}
