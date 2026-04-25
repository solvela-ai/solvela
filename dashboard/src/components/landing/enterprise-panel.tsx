import { Building2, Users, Wallet, KeyRound, FileClock, LineChart } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { DOCS_URL } from './config'
import { diagramPalette as C } from '@/lib/diagram-colors'

interface ListFeature {
  icon: LucideIcon
  title: string
  desc: string
  mono: string
  href?: string
}

// Organizations is the featured hero-card; the remaining five are a compact list.
const LIST_FEATURES: ListFeature[] = [
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
    <section
      aria-labelledby="enterprise-heading"
      className="border-t border-border/60"
    >
      <div className="mx-auto max-w-[1280px] px-6 py-16 lg:py-24">
        <div className="flex flex-col gap-3 pb-10 sm:flex-row sm:items-end sm:justify-between">
          <div className="flex flex-col gap-3">
            <span className="eyebrow">for teams &amp; enterprise</span>
            <h2
              id="enterprise-heading"
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

        {/* Asymmetric split: one hero feature card + five compact list rows.
            Mirrors the 0.55/0.45 rhythm of the escrow panel to break the
            "six identical tiles" pattern. */}
        <div className="grid items-stretch gap-6 lg:grid-cols-[minmax(0,0.55fr)_minmax(0,0.45fr)]">
          <OrganizationsHeroCard />
          <ul
            className="flex h-full list-none flex-col justify-stretch rounded-lg border border-border bg-[var(--card)]"
            role="list"
          >
            {LIST_FEATURES.map((f, idx) => (
              <ListRow
                key={f.title}
                feature={f}
                isLast={idx === LIST_FEATURES.length - 1}
              />
            ))}
          </ul>
        </div>
      </div>
    </section>
  )
}

function OrganizationsHeroCard() {
  return (
    <a
      href={`${DOCS_URL}/docs/enterprise/orgs`}
      className="group relative flex h-full flex-col justify-between overflow-hidden rounded-lg border border-border bg-[var(--card)] p-8 transition-colors hover:bg-[var(--color-bg-surface-hover)]"
    >
      <div className="flex flex-col gap-4">
        <div className="flex items-center gap-3">
          <Building2 className="h-5 w-5 text-[var(--accent-salmon)]" aria-hidden />
          <span className="font-mono text-[10px] uppercase tracking-[0.22em] text-text-faint">
            foundation primitive
          </span>
        </div>
        <h3 className="font-display text-[clamp(1.5rem,2.6vw,2rem)] font-semibold leading-[1.1] tracking-[-0.01em] text-foreground">
          Organizations
        </h3>
        <p className="max-w-[42ch] text-[15px] leading-[1.55] text-muted-foreground">
          Multi-tenant orgs with parent / child hierarchy. One gateway deployment
          serves every customer in isolation — spend, keys, and quotas scoped at
          the org boundary.
        </p>
      </div>

      <OrgTreeDiagram />

      <div className="flex items-center justify-between gap-3 pt-6">
        <span className="inline-flex items-center rounded border border-border/60 px-2 py-1 font-mono text-[11px] text-text-faint">
          POST /v1/orgs
        </span>
        <span className="font-mono text-[10px] uppercase tracking-[0.18em] text-text-faint opacity-0 transition-opacity group-hover:opacity-100">
          read docs →
        </span>
      </div>
    </a>
  )
}

// Pure SVG — parent/child tree abstracting an org with three teams
// and five wallets total. viewBox sized so 14pt labels stay readable on
// narrow viewports (≥375px → ≈11px rendered).
function OrgTreeDiagram() {
  // viewBox 560×180 accommodates three 170-wide team nodes. "team · research"
  // (15 chars @ fontSize 14) measures ~126 viewBox units — node widths were
  // raised from 150 → 170 so that text clears the right edge.
  return (
    <svg
      viewBox="0 0 560 180"
      aria-hidden
      className="my-6 h-auto w-full max-w-[560px] self-center"
      preserveAspectRatio="xMidYMid meet"
    >
      {/* connectors */}
      <g stroke={C.nodeStroke} strokeWidth="1" fill="none">
        <path d="M 280 39 V 64 H 100 V 88" />
        <path d="M 280 39 V 88" />
        <path d="M 280 39 V 64 H 460 V 88" />
        <path d="M 100 122 V 146 H 50  V 160" />
        <path d="M 100 146 H 150 V 160" />
        <path d="M 280 122 V 160" />
        <path d="M 460 122 V 146 H 410 V 160" />
        <path d="M 460 146 H 510 V 160" />
      </g>

      {/* parent org */}
      <Node cx={280} cy={22} label="org · acme" primary />
      {/* teams */}
      <Node cx={100} cy={105} label="team · finance" />
      <Node cx={280} cy={105} label="team · ops" />
      <Node cx={460} cy={105} label="team · research" />
      {/* wallets */}
      <Leaf cx={50} cy={166} />
      <Leaf cx={150} cy={166} />
      <Leaf cx={280} cy={166} />
      <Leaf cx={410} cy={166} />
      <Leaf cx={510} cy={166} />
    </svg>
  )
}

function Node({
  cx,
  cy,
  label,
  primary,
}: {
  cx: number
  cy: number
  label: string
  primary?: boolean
}) {
  const w = 170
  const h = 34
  const x = cx - w / 2
  const y = cy - h / 2
  return (
    <g>
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx="6"
        fill={primary ? C.nodeBlackish : C.nodeFrontMid}
        stroke={primary ? C.accentGold : C.nodeStroke}
        strokeWidth="1"
        strokeDasharray={primary ? '4 3' : undefined}
      />
      <circle cx={x + 14} cy={cy} r="3" fill={primary ? C.accentGold : C.neutralText} />
      <text
        x={x + 26}
        y={cy + 5}
        fontFamily="'JetBrains Mono', monospace"
        fontSize="14"
        fill={C.headingText}
        letterSpacing="0.5"
      >
        {label}
      </text>
    </g>
  )
}

function Leaf({ cx, cy }: { cx: number; cy: number }) {
  return (
    <g>
      <rect
        x={cx - 16}
        y={cy - 9}
        width={32}
        height={18}
        rx="3"
        fill={C.nodeFrontDark}
        stroke={C.nodeStroke}
        strokeWidth="1"
      />
      <circle cx={cx} cy={cy} r="2.5" fill={C.accentSalmon} />
    </g>
  )
}

function ListRow({ feature, isLast }: { feature: ListFeature; isLast: boolean }) {
  const Icon = feature.icon
  const body = (
    <div
      className={
        'group relative flex items-start gap-4 p-5 transition-colors hover:bg-[var(--color-bg-surface-hover)]' +
        (isLast ? '' : ' border-b border-border/60')
      }
    >
      {/* salmon accent bar on hover */}
      <span
        aria-hidden
        className="pointer-events-none absolute left-0 top-3 bottom-3 w-[2px] origin-top scale-y-0 bg-[var(--accent-salmon)] transition-transform duration-200 group-hover:scale-y-100"
      />
      <Icon className="mt-0.5 h-4 w-4 flex-shrink-0 text-text-faint transition-colors group-hover:text-[var(--accent-salmon)]" aria-hidden />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <div className="flex items-baseline justify-between gap-3">
          <h3 className="min-w-0 truncate font-display text-[15px] font-semibold leading-[1.3] text-foreground">
            {feature.title}
          </h3>
          <span className="flex-shrink-0 whitespace-nowrap font-mono text-[10px] text-text-faint">
            {feature.mono}
          </span>
        </div>
        <p className="text-[13px] leading-[1.5] text-muted-foreground">
          {feature.desc}
        </p>
      </div>
    </div>
  )
  return (
    <li>
      {feature.href ? (
        <a href={feature.href} className="block">
          {body}
        </a>
      ) : (
        body
      )}
    </li>
  )
}
