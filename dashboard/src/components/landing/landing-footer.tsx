import Link from 'next/link'
import { VERSION, GITHUB_URL, DOCS_URL, APP_URL, ESCROW_PROGRAM_ID } from './config'

export function LandingFooter() {
  return (
    <footer className="border-t border-border/60 bg-[var(--background)]">
      <div className="mx-auto max-w-[1280px] px-6 py-14">
        <div className="grid gap-10 md:grid-cols-[minmax(0,1.1fr)_repeat(4,minmax(0,0.7fr))]">
          <div className="flex flex-col gap-3">
            <Link href="/" className="inline-flex items-center gap-2 text-foreground">
              <span
                aria-hidden
                className="inline-block h-3 w-3 rounded-full bg-[var(--accent-salmon)]"
              />
              <span className="font-display text-[18px] font-semibold">solvela</span>
            </Link>
            <p className="max-w-[34ch] text-[13px] leading-[1.55] text-muted-foreground">
              Solana-native x402 gateway with trustless escrow. Built for agents,
              not dashboards.
            </p>
            <div className="mt-3 flex items-center gap-2 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint">
              <span className="rounded border border-border px-1.5 py-0.5">{VERSION}</span>
              <span className="rounded border border-border px-1.5 py-0.5 text-[var(--color-success)]">
                mainnet
              </span>
            </div>
          </div>

          <FooterColumn
            title="product"
            links={[
              { label: 'docs', href: DOCS_URL },
              { label: 'app', href: APP_URL },
              { label: 'quickstart', href: `${DOCS_URL}/docs/quickstart` },
              { label: 'changelog', href: `${DOCS_URL}/docs/changelog` },
            ]}
          />

          <FooterColumn
            title="protocol"
            links={[
              { label: 'x402', href: `${DOCS_URL}/docs/concepts/x402` },
              { label: 'escrow', href: `${DOCS_URL}/docs/concepts/escrow` },
              { label: 'smart router', href: `${DOCS_URL}/docs/concepts/smart-router` },
              { label: 'a2a agent-card', href: `${DOCS_URL}/docs/api` },
            ]}
          />

          <FooterColumn
            title="sdks"
            links={[
              { label: 'typescript', href: `${DOCS_URL}/docs/sdks/typescript` },
              { label: 'python', href: `${DOCS_URL}/docs/sdks/python` },
              { label: 'go', href: `${DOCS_URL}/docs/sdks/go` },
              { label: 'rust cli', href: `${DOCS_URL}/docs/sdks/rust` },
              { label: 'mcp server', href: `${DOCS_URL}/docs/sdks/mcp` },
            ]}
          />

          <FooterColumn
            title="solvela"
            links={[
              { label: 'github', href: GITHUB_URL },
              { label: 'status', href: `${APP_URL}/overview` },
              { label: 'security', href: `${DOCS_URL}/docs/operations/security` },
            ]}
          />
        </div>

        <div className="mt-12 flex flex-col gap-3 border-t border-border/60 pt-6 font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint md:flex-row md:items-center md:justify-between">
          <span>
            © 2026 Solvela · escrow pda · {ESCROW_PROGRAM_ID}…
          </span>
          <span className="text-[var(--accent-salmon)]">
            built in rust · settled on solana · priced in usdc
          </span>
        </div>
      </div>
    </footer>
  )
}

interface FooterColumnProps {
  title: string
  links: { label: string; href: string }[]
}

function FooterColumn({ title, links }: FooterColumnProps) {
  return (
    <div className="flex flex-col gap-1">
      <span className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint">
        {title}
      </span>
      {links.map((l) => (
        <a
          key={l.label}
          href={l.href}
          className="inline-flex h-11 items-center text-[13px] text-muted-foreground transition-colors hover:text-foreground"
        >
          {l.label}
        </a>
      ))}
    </div>
  )
}
