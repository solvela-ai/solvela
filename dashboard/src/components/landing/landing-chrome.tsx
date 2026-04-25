import Link from 'next/link'
import { GITHUB_URL, VERSION } from './config'

// This file stays a server component. The ticker (which needs an
// IntersectionObserver) lives in its own `'use client'` file so the
// top-strip never ships JS to the client.
export { LandingTicker } from './landing-ticker'

const NAV_LINK_CLASS =
  'inline-flex h-11 items-center px-2 -mx-2 hover:text-foreground transition-colors'

export function LandingTopStrip() {
  return (
    <header className="relative z-20 border-b border-border/60">
      <div className="mx-auto flex h-11 max-w-[1280px] items-center gap-6 px-6 font-mono text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
        <Link
          href="/"
          className="inline-flex h-11 items-center gap-2 px-2 -mx-2 text-foreground"
        >
          <span
            aria-hidden
            className="inline-block h-2.5 w-2.5 rounded-full bg-[var(--accent-salmon)]"
          />
          <span className="font-semibold">solvela</span>
        </Link>
        <span aria-hidden className="hidden sm:inline text-text-faint">/</span>
        <span className="hidden sm:inline">{VERSION}</span>
        <span aria-hidden className="hidden md:inline text-text-faint">/</span>
        <span className="hidden md:inline text-[var(--color-success)]">mainnet</span>

        <nav className="ml-auto flex items-center gap-3 sm:gap-5" aria-label="primary">
          <a href="https://docs.solvela.ai" className={NAV_LINK_CLASS}>
            docs
          </a>
          <a href="https://app.solvela.ai" className={NAV_LINK_CLASS}>
            app
          </a>
          <a href={GITHUB_URL} className={NAV_LINK_CLASS}>
            github
          </a>
        </nav>
      </div>
    </header>
  )
}
