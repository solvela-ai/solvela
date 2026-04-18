import { Building2, Users, Wallet, KeyRound, FileClock, LineChart } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { DOCS_URL } from './config'

interface Feature {
  icon: LucideIcon
  title: string
  desc: string
  mono: string
  href?: string
}

const FEATURES: Feature[] = [
  {
    icon: Building2,
    title: 'Organizations',
    desc: 'Multi-tenant orgs with parent/child hierarchy. One deployment serves every customer in isolation.',
    mono: 'POST /v1/orgs',
    href: `${DOCS_URL}/docs/enterprise/orgs`,
  },
  {
    icon: Users,
    title: 'Teams & roles',
    desc: 'Team scoping with admin / member roles. Route spend to the right wallet, enforce policy per team.',
    mono: 'POST /v1/teams',
    href: `${DOCS_URL}/docs/enterprise/teams`,
  },
  {
    icon: Wallet,
    title: 'Per-wallet budgets',
    desc: 'Hard spend caps in USDC per wallet, per team, per org. Block at the gateway before the provider hits.',
    mono: 'POST /v1/budgets',
    href: `${DOCS_URL}/docs/enterprise/budgets`,
  },
  {
    icon: KeyRound,
    title: 'Scoped API keys',
    desc: 'Prefix-signed keys with configurable scope, expiry, and wallet binding. Rotate without redeploying agents.',
    mono: 'POST /v1/api-keys',
    href: `${DOCS_URL}/docs/enterprise/api-keys`,
  },
  {
    icon: FileClock,
    title: 'Audit log',
    desc: 'Every request, every 402, every claim — fire-and-forget write to Postgres. Export, ingest, comply.',
    mono: 'GET /v1/audit',
    href: `${DOCS_URL}/docs/enterprise/audit`,
  },
  {
    icon: LineChart,
    title: 'Usage analytics',
    desc: 'Spend by model, wallet, org, or time window. Cost attribution at the API boundary, not the invoice.',
    mono: 'GET /v1/analytics',
    href: `${DOCS_URL}/docs/enterprise/analytics`,
  },
]

export function EnterprisePanel() {
  return (
    <section className="border-t border-border/60">
      <div className="mx-auto max-w-[1280px] px-6 py-16 lg:py-24">
        <div className="flex flex-col gap-3 pb-10 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex flex-col gap-3">
            <span className="eyebrow">for teams & enterprise</span>
            <h2
              className="font-display leading-[1.0] tracking-[-0.02em] text-foreground"
              style={{ fontSize: 'clamp(2rem, 4vw, 3.25rem)', fontWeight: 600 }}
            >
              Multi-tenant from the base layer.
            </h2>
            <p className="max-w-[54ch] text-[15px] leading-[1.55] text-muted-foreground">
              Orgs, teams, per-wallet budgets, scoped keys, and a full audit
              trail — first-class gateway primitives, not a bolt-on.
            </p>
          </div>
          <a
            href={`${DOCS_URL}/docs/enterprise`}
            className="inline-flex items-center gap-2 self-start rounded-md border border-border px-4 py-2.5 font-mono text-[11px] uppercase tracking-[0.16em] text-foreground transition-colors hover:border-[var(--accent-salmon)] hover:text-[var(--accent-salmon)]"
          >
            enterprise docs →
          </a>
        </div>

        <div className="grid gap-px overflow-hidden rounded-lg border border-border bg-[var(--border)] md:grid-cols-2 lg:grid-cols-3">
          {FEATURES.map((f) => {
            const Icon = f.icon
            const content = (
              <div className="group flex h-full flex-col gap-4 bg-[var(--card)] p-6 transition-colors duration-200 hover:bg-[var(--bg-surface-raised)]">
                <div className="flex items-center justify-between">
                  <div className="flex h-10 w-10 items-center justify-center rounded-md border border-border bg-[var(--popover)] text-foreground">
                    <Icon className="h-4 w-4" aria-hidden />
                  </div>
                  <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint opacity-0 transition-opacity group-hover:opacity-100">
                    docs →
                  </span>
                </div>
                <div className="flex flex-col gap-2">
                  <h3 className="font-display text-[17px] font-semibold leading-[1.2] text-foreground">
                    {f.title}
                  </h3>
                  <p className="text-[14px] leading-[1.55] text-muted-foreground">
                    {f.desc}
                  </p>
                </div>
                <span className="mt-auto inline-flex w-fit rounded border border-border/60 px-1.5 py-0.5 font-mono text-[10px] text-text-faint">
                  {f.mono}
                </span>
              </div>
            )
            return f.href ? (
              <a key={f.title} href={f.href} className="h-full">
                {content}
              </a>
            ) : (
              <div key={f.title} className="h-full">
                {content}
              </div>
            )
          })}
        </div>
      </div>
    </section>
  )
}
