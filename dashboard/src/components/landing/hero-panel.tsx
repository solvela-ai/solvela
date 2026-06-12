'use client'

import Link from 'next/link'
import { ArrowRight } from 'lucide-react'
import { motion, useReducedMotion } from 'motion/react'
import { HeroHandshake } from './hero-handshake'
import { SettlementReadout } from './settlement-readout'
import { QUICKSTART_URL, DOCS_URL, ESCROW_PROGRAM_ID } from './config'

const EASE_OUT = [0.16, 1, 0.3, 1] as const

// The three verbs of the A2A narrative. Each rises in on its own beat; the
// final verb carries the wonk-heavy weight as the visual anchor.
const VERBS = ['discover.', 'negotiate.', 'pay.'] as const

export function HeroPanel() {
  const reduced = useReducedMotion()

  return (
    <section
      aria-labelledby="hero-heading"
      className="hero-grid-v2 relative overflow-hidden"
    >
      <div className="relative z-[1] mx-auto max-w-[1320px] px-6 pb-14 pt-16 sm:pt-20 lg:pb-20 lg:pt-24">
        {/* Top meta line — brutalist running header, mono */}
        <div className="mb-10 flex flex-wrap items-center gap-x-5 gap-y-2 font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint lg:mb-14">
          <span className="text-[var(--accent-salmon)]">agent-to-agent commerce</span>
          <span aria-hidden className="text-border">/</span>
          <span>solana-native x402 gateway</span>
          <span aria-hidden className="hidden text-border sm:inline">/</span>
          <span className="hidden sm:inline">usdc-spl · mainnet</span>
        </div>

        <div className="grid items-end gap-12 lg:grid-cols-[minmax(0,1.18fr)_minmax(0,0.82fr)] lg:gap-14">
          {/* ── left: the kinetic headline column ─────────────────── */}
          <div className="flex flex-col">
            <h1 id="hero-heading" className="sr-only">
              Agents discover, negotiate, and pay — end to end, no humans.
            </h1>

            {/* The headline as a stacked verb-list. Oversized Fraunces wonk;
                each verb in its own clipping row so it rises from the baseline.
                aria-hidden — the sr-only h1 above carries the accessible name. */}
            <div aria-hidden className="flex flex-col">
              <span className="display-calm mb-1 text-[clamp(0.95rem,1.5vw,1.15rem)] font-normal tracking-[0.01em] text-muted-foreground">
                agents
              </span>
              {VERBS.map((verb, i) => (
                <span
                  key={verb}
                  className="block overflow-hidden"
                  style={{ lineHeight: 0.92 }}
                >
                  <motion.span
                    className="display-wonk block text-[var(--heading-color)] will-change-transform"
                    style={{
                      fontSize: 'clamp(3.4rem, 11vw, 9rem)',
                      fontWeight: i === 2 ? 620 : 540,
                      fontVariationSettings: `'opsz' 144, 'SOFT' ${
                        i === 2 ? 0 : 6
                      }, 'WONK' 1`,
                    }}
                    initial={reduced ? false : { y: '108%' }}
                    animate={reduced ? undefined : { y: '0%' }}
                    transition={{
                      duration: 0.9,
                      ease: EASE_OUT,
                      delay: 0.08 + i * 0.11,
                    }}
                  >
                    {verb}
                  </motion.span>
                </span>
              ))}
            </div>

            <motion.p
              className="mt-7 max-w-[48ch] text-[16px] leading-[1.6] text-muted-foreground sm:text-[17px]"
              initial={reduced ? false : { opacity: 0, y: 18 }}
              animate={reduced ? undefined : { opacity: 1, y: 0 }}
              transition={{ duration: 0.7, ease: EASE_OUT, delay: 0.5 }}
            >
              End to end, no humans in the loop. Your agent finds a model,
              gets a 402 quote, signs a USDC payment, and settles on-chain —
              and the gateway only claims what the provider actually delivered.
            </motion.p>

            <motion.div
              className="mt-8 flex flex-wrap items-center gap-3"
              initial={reduced ? false : { opacity: 0, y: 18 }}
              animate={reduced ? undefined : { opacity: 1, y: 0 }}
              transition={{ duration: 0.7, ease: EASE_OUT, delay: 0.6 }}
            >
              <a
                href={QUICKSTART_URL}
                className="group inline-flex items-center gap-2 rounded-md bg-foreground px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-[var(--primary-foreground)] transition-all duration-150 hover:-translate-y-[1px] hover:bg-[var(--heading-color)] active:translate-y-0"
              >
                start building
                <ArrowRight className="h-3.5 w-3.5 transition-transform group-hover:translate-x-0.5" />
              </a>
              <Link
                href={DOCS_URL}
                className="inline-flex items-center gap-2 rounded-md border border-border px-5 py-3 font-mono text-[12px] uppercase tracking-[0.16em] text-foreground transition-colors hover:border-[var(--accent-salmon)] hover:text-[var(--accent-salmon)]"
              >
                read docs
              </Link>
              <span className="ml-1 hidden font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint md:inline">
                · no accounts · no api keys · just wallets
              </span>
            </motion.div>
          </div>

          {/* ── right: trust badge + live handshake artifact ──────── */}
          <motion.div
            className="flex flex-col gap-4 lg:pb-2"
            initial={reduced ? false : { opacity: 0, y: 24 }}
            animate={reduced ? undefined : { opacity: 1, y: 0 }}
            transition={{ duration: 0.8, ease: EASE_OUT, delay: 0.35 }}
          >
            {/* escrow trust anchor — the single most important claim */}
            <div className="flex items-start gap-3 rounded-md border border-[var(--color-border-emphasis)] bg-[var(--tint-gold-soft)] px-4 py-3">
              <span
                aria-hidden
                className="mt-[5px] inline-block h-2 w-2 shrink-0 rounded-full bg-[var(--accent-gold)] shadow-[0_0_0_3px_var(--tint-gold-medium)]"
              />
              <div className="flex flex-col gap-0.5">
                <span className="font-mono text-[11px] uppercase tracking-[0.16em] text-[var(--accent-gold)]">
                  the only x402 gateway with trustless on-chain escrow
                </span>
                <span className="font-mono text-[10px] tracking-[0.04em] text-text-faint">
                  mainnet program {ESCROW_PROGRAM_ID}…
                </span>
              </div>
            </div>

            <HeroHandshake />
            <p className="pl-1 font-mono text-[10px] uppercase tracking-[0.2em] text-text-faint">
              live 402 handshake · agent ⇄ gateway ⇄ escrow
            </p>
          </motion.div>
        </div>
      </div>

      {/* full-bleed settlement readout — the texture line under the hero */}
      <div className="relative z-[1] border-t border-border/60 bg-[var(--popover)] py-2.5">
        <SettlementReadout />
      </div>
    </section>
  )
}
