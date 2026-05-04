/**
 * Sponsor page — `/sponsor` on the apex domain (solvela.ai/sponsor).
 *
 * Linked from `.github/FUNDING.yml`. Audience is anyone who clicked the
 * "Sponsor" button on GitHub or landed here from a maintainer's footer.
 * Goal is to convert that interest into either:
 *
 *   1. a recurring sponsorship (GitHub Sponsors / Polar.sh) — funds infra
 *      and lets the gateway stay online for the BSL Additional Use Grant
 *      (free-for-sub-$1M-revenue, free-for-first-party) population, or
 *   2. a commercial license — for the rare case where someone lands here
 *      but actually needs the BSL exception, not a donation.
 *
 * Plain SSR. No client state. Designed to match `/metrics` visually so the
 * "evidence pages" feel like one surface.
 */

import type { Metadata } from 'next';
import Link from 'next/link';
import { LandingTopStrip } from '@/components/landing/landing-chrome';
import { LandingFooter } from '@/components/landing/landing-footer';
import { TerminalCard } from '@/components/ui/terminal-card';

const GITHUB_SPONSORS_URL = 'https://github.com/sponsors/solvela-ai';
const POLAR_URL = 'https://polar.sh/solvela-ai';
const COMMERCIAL_URL = 'https://docs.solvela.ai/docs/enterprise/commercial-license';
const METRICS_URL = 'https://solvela.ai/metrics';
const CONTACT_EMAIL = 'kd@sky64.io';

// Tiers are deliberate. Each one is named after what it actually pays for —
// not a vanity ladder. If a sponsor asks "what does $X cover?" we want the
// answer on the tin.
const SPONSOR_TIERS = [
  {
    name: 'Supporter',
    monthly: '$10',
    covers: 'A week of Solana RPC quota under the public free-tier ceiling.',
    perks: ['Listed in BACKERS.md', 'Sponsor badge on your GitHub profile'],
  },
  {
    name: 'Operator',
    monthly: '$50',
    covers: 'A month of Fly.io dyno + Upstash Redis + Vercel hobby tier.',
    perks: [
      'Listed in BACKERS.md and the README',
      'Priority triage on issues you open',
      'Quarterly office-hours invitation',
    ],
  },
  {
    name: 'Infrastructure',
    monthly: '$250',
    covers:
      'A month of paid-tier RPC (Helius / Triton) + monitoring + spare capacity for traffic bursts.',
    perks: [
      'All Operator perks',
      'Logo on the homepage sponsors strip',
      'Direct Slack/Discord with the maintainer',
    ],
  },
  {
    name: 'Underwriter',
    monthly: '$1,000',
    covers:
      'Audit prep, contributor honoraria, and the kind of headroom that keeps the lights on without rent-seeking from users.',
    perks: [
      'All Infrastructure perks',
      'Quarterly roadmap call (you tell us what to build, we tell you what we will)',
      'Pre-publication notice on security advisories',
    ],
  },
] as const;

// Categories of cost that sponsorship actually pays. Mirrors the
// real Fly.io / Vercel / Upstash / RPC line items so sponsors know
// they're funding infrastructure, not a salary.
const COST_LINES = [
  {
    label: 'Solana RPC',
    detail:
      'Helius / Triton paid tier when free-tier limits are exceeded. Largest variable cost; scales with verified payments.',
  },
  {
    label: 'Gateway hosting',
    detail: 'Fly.io app + autoscale headroom for the api.solvela.ai endpoint.',
  },
  {
    label: 'Cache + queue',
    detail: 'Upstash Redis for x402 nonce store, idempotency keys, rate limits.',
  },
  {
    label: 'Frontend + docs',
    detail: 'Vercel Pro tier for solvela.ai / docs.solvela.ai / app.solvela.ai.',
  },
  {
    label: 'Security',
    detail:
      'Quarterly cargo-audit / cargo-deny review; one external review per release of the Anchor escrow program.',
  },
  {
    label: 'Domains, certs, monitoring',
    detail: 'Registrar fees, BetterUptime, Sentry hobby, log retention.',
  },
] as const;

export const metadata: Metadata = {
  title: 'Sponsor Solvela',
  description:
    'Solvela is maintained by a small team. Sponsorship covers Fly.io, Vercel, Upstash, and Solana RPC bills, and lets the gateway stay free for everyone under the BSL Additional Use Grant.',
  alternates: { canonical: 'https://solvela.ai/sponsor' },
  robots: { index: true, follow: true },
};

export default function SponsorPage() {
  return (
    <main className="dark min-h-screen bg-[var(--background)] text-foreground">
      <LandingTopStrip />

      <div className="mx-auto max-w-[1280px] px-6 py-12 space-y-10">
        {/* Header */}
        <header className="space-y-3">
          <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-text-tertiary">
            solvela / sponsor
          </p>
          <h1 className="font-display text-4xl md:text-5xl font-semibold leading-tight">
            Keep the gateway lit.
          </h1>
          <p className="max-w-[60ch] text-[15px] leading-[1.6] text-muted-foreground">
            Solvela is maintained by a small team. The hosted gateway at{' '}
            <code className="font-mono text-xs">api.solvela.ai</code> is free
            to use under the BSL Additional Use Grant for any organization
            below USD&nbsp;$1M in attributable revenue. Sponsorship covers
            the bills that make &ldquo;free&rdquo; possible.
          </p>
          <div className="flex flex-wrap items-center gap-3 pt-1">
            <a
              href={GITHUB_SPONSORS_URL}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-10 items-center rounded border border-border px-4 font-mono text-xs uppercase tracking-[0.14em] text-foreground transition-colors hover:bg-surface-hover"
            >
              github sponsors ↗
            </a>
            <a
              href={POLAR_URL}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-10 items-center rounded border border-border px-4 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
            >
              polar.sh ↗
            </a>
            <Link
              href="/metrics"
              className="inline-flex h-10 items-center rounded border border-border px-4 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
            >
              see metrics →
            </Link>
          </div>
        </header>

        {/* What sponsorship actually covers */}
        <section aria-labelledby="costs-heading" className="space-y-3">
          <p id="costs-heading" className="eyebrow">where the money goes</p>
          <TerminalCard title="cost.lines" accentDot>
            <ul className="divide-y divide-border/40 text-sm">
              {COST_LINES.map((line) => (
                <li key={line.label} className="flex flex-col gap-1 py-3 sm:flex-row sm:items-baseline sm:gap-6">
                  <span className="min-w-[180px] font-mono text-text-secondary">
                    {line.label}
                  </span>
                  <span className="text-text-tertiary leading-relaxed">{line.detail}</span>
                </li>
              ))}
            </ul>
            <p className="mt-4 text-xs text-text-tertiary leading-relaxed">
              Sponsorship is not equity, is not a contract, and does not
              entitle you to support obligations the maintainer cannot
              honor. It is a tip jar with structure.
            </p>
          </TerminalCard>
        </section>

        {/* Tiers */}
        <section aria-labelledby="tiers-heading" className="space-y-3">
          <p id="tiers-heading" className="eyebrow">tiers</p>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-4">
            {SPONSOR_TIERS.map((tier) => (
              <TerminalCard key={tier.name} title={tier.name.toLowerCase()}>
                <p className="metric-md">{tier.monthly}<span className="ml-1 text-xs text-text-tertiary font-mono">/mo</span></p>
                <p className="mt-2 text-xs text-text-tertiary leading-relaxed">
                  {tier.covers}
                </p>
                <ul className="mt-4 space-y-1.5 text-xs text-text-secondary">
                  {tier.perks.map((perk) => (
                    <li key={perk} className="flex gap-2">
                      <span aria-hidden className="text-text-faint">·</span>
                      <span className="leading-relaxed">{perk}</span>
                    </li>
                  ))}
                </ul>
                <a
                  href={GITHUB_SPONSORS_URL}
                  target="_blank"
                  rel="noreferrer"
                  className="mt-4 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-[11px] uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
                >
                  sponsor at this tier ↗
                </a>
              </TerminalCard>
            ))}
          </div>
        </section>

        {/* When you actually want a commercial license, not a sponsorship */}
        <section aria-labelledby="vs-heading" className="space-y-3">
          <p id="vs-heading" className="eyebrow">sponsorship vs commercial license</p>
          <TerminalCard title="which.do.i.need">
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border/60 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-text-tertiary">
                    <th className="py-2 pr-4">your situation</th>
                    <th className="py-2 pr-4">what you want</th>
                    <th className="py-2">link</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/30 font-mono text-xs">
                  <tr>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Calling api.solvela.ai or self-hosting under $1M revenue
                    </td>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Optional sponsorship — keeps the public gateway online
                    </td>
                    <td className="py-3 text-text-tertiary">
                      <a href={GITHUB_SPONSORS_URL} target="_blank" rel="noreferrer" className="underline underline-offset-2 hover:text-foreground">
                        sponsor ↗
                      </a>
                    </td>
                  </tr>
                  <tr>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Hosting the gateway as a managed service for third parties
                    </td>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Commercial license required (BSL trigger)
                    </td>
                    <td className="py-3 text-text-tertiary">
                      <a href={COMMERCIAL_URL} target="_blank" rel="noreferrer" className="underline underline-offset-2 hover:text-foreground">
                        pricing ↗
                      </a>
                    </td>
                  </tr>
                  <tr>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Annual revenue from the gateway exceeds $1M
                    </td>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Commercial license required (revenue-cap trigger)
                    </td>
                    <td className="py-3 text-text-tertiary">
                      <a href={COMMERCIAL_URL} target="_blank" rel="noreferrer" className="underline underline-offset-2 hover:text-foreground">
                        pricing ↗
                      </a>
                    </td>
                  </tr>
                  <tr>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Embedding any non-gateway crate or SDK
                    </td>
                    <td className="py-3 pr-4 text-text-secondary leading-relaxed">
                      Nothing — Apache-2.0 / MIT, just keep notices
                    </td>
                    <td className="py-3 text-text-tertiary">
                      <a href={METRICS_URL} className="underline underline-offset-2 hover:text-foreground">
                        license split →
                      </a>
                    </td>
                  </tr>
                </tbody>
              </table>
            </div>
          </TerminalCard>
        </section>

        {/* CTA */}
        <section aria-labelledby="cta-heading" className="space-y-3">
          <p id="cta-heading" className="eyebrow">talk to us</p>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <TerminalCard title="recurring">
              <p className="text-sm text-text-secondary leading-relaxed">
                GitHub Sponsors handles billing, receipts, and corporate
                cards. Cancel any time.
              </p>
              <a
                href={GITHUB_SPONSORS_URL}
                target="_blank"
                rel="noreferrer"
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                sponsor ↗
              </a>
            </TerminalCard>
            <TerminalCard title="one.time">
              <p className="text-sm text-text-secondary leading-relaxed">
                Polar.sh accepts one-time and crypto payments if a
                recurring charge isn&rsquo;t a fit.
              </p>
              <a
                href={POLAR_URL}
                target="_blank"
                rel="noreferrer"
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                polar.sh ↗
              </a>
            </TerminalCard>
            <TerminalCard title="big.deal">
              <p className="text-sm text-text-secondary leading-relaxed">
                Acquihire interest, source-buyout, exclusive license, or
                an underwriter check above the listed tiers? Email
                directly.
              </p>
              <a
                href={`mailto:${CONTACT_EMAIL}`}
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                {CONTACT_EMAIL} ↗
              </a>
            </TerminalCard>
          </div>
        </section>
      </div>

      <LandingFooter />
    </main>
  );
}
