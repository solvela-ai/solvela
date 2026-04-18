import Link from 'next/link'
import { GITHUB_URL, VERSION } from './config'

export function LandingTopStrip() {
  return (
    <header className="relative z-20 border-b border-border/60">
      <div className="mx-auto flex h-11 max-w-[1280px] items-center gap-6 px-6 font-mono text-[11px] uppercase tracking-[0.14em] text-text-tertiary">
        <Link href="/" className="flex items-center gap-2 text-foreground">
          <span
            aria-hidden
            className="inline-block h-2.5 w-2.5 rounded-full bg-[var(--accent-salmon)]"
          />
          <span className="font-semibold">solvela</span>
        </Link>
        <span className="hidden sm:inline text-text-faint">/</span>
        <span className="hidden sm:inline">{VERSION}</span>
        <span className="hidden md:inline text-text-faint">/</span>
        <span className="hidden md:inline text-[var(--color-success)]">mainnet</span>

        <nav className="ml-auto flex items-center gap-5">
          <a
            href="https://docs.solvela.ai"
            className="hover:text-foreground transition-colors"
          >
            docs
          </a>
          <a
            href="https://app.solvela.ai"
            className="hover:text-foreground transition-colors"
          >
            app
          </a>
          <a
            href={GITHUB_URL}
            className="hover:text-foreground transition-colors"
          >
            github
          </a>
        </nav>
      </div>
    </header>
  )
}

export function LandingTicker() {
  const items = [
    'ESCROW · 9neDHouXgEgHZDde5Sp',
    'p50 · 38ms',
    'p99 · 184ms',
    '26 models · 5 providers',
    '5% flat fee',
    'mainnet · x402 · usdc-spl',
    'a2a · agent-card · /.well-known',
  ]
  const track = [...items, ...items]
  return (
    <div className="relative z-10 overflow-hidden border-y border-border/60 bg-[var(--popover)]">
      <div className="ticker-track py-2 font-mono text-[11px] uppercase tracking-[0.18em] text-text-faint">
        {track.map((t, i) => (
          <span key={i} className="inline-flex items-center gap-3">
            <span className="inline-block h-1 w-1 rounded-full bg-[var(--accent-salmon)]" />
            {t}
          </span>
        ))}
      </div>
    </div>
  )
}
