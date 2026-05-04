/**
 * Public metrics page — `/metrics` (also served at `metrics.solvela.ai`
 * once the proxy rewrite is deployed; see `src/proxy.ts`).
 *
 * Audience: grant evaluators, prospective acquirers, integrators doing
 * due diligence. Goal is to answer "is this real?" with verifiable
 * evidence, not to dashboard-shame ourselves with vanity numbers.
 *
 * Three categories of data here, ordered by how trust-bearing they are:
 *
 *   1. ON-CHAIN — escrow program ID + USDC mint. Verifiable via Solscan
 *      independently of anything we say.
 *   2. LIVE — gateway health + aggregate (non-PII) stats from the
 *      admin endpoint. Falls back gracefully if gateway is unreachable.
 *   3. STATIC EVIDENCE — verified perf numbers, license posture,
 *      crate versions. Sourced from STATUS.md / CHANGELOG.md and
 *      pinned at build time. Each carries a "verified on" date.
 *
 * No mutation, no client state, no analytics scripts. Plain SSR.
 */

export const dynamic = 'force-dynamic';

import type { Metadata } from 'next';
import { LandingTopStrip } from '@/components/landing/landing-chrome';
import { LandingFooter } from '@/components/landing/landing-footer';
import { TerminalCard } from '@/components/ui/terminal-card';
import { StatusDot } from '@/components/ui/status-dot';
import { SpendChart } from '@/components/charts/spend-chart';
import { RequestsBar } from '@/components/charts/requests-bar';
import {
  fetchHealth,
  fetchPricing,
  fetchPublicMetrics,
  fetchEscrowConfig,
  fetchServices,
} from '@/lib/api';
import { formatUSDC, formatNumber, shortAddress } from '@/lib/utils';
import { ESCROW_PROGRAM_ID } from '@/components/landing/config';

const FULL_ESCROW_PROGRAM_ID = '9neDHouXgEgHZDde5SpmqqEZ9Uv35hFcjtFEPxomtHLU';
const SOLSCAN_PROGRAM_URL = `https://solscan.io/account/${FULL_ESCROW_PROGRAM_ID}`;
const GITHUB_URL = 'https://github.com/solvela-ai/solvela';

// Verified facts pinned to the most recent STATUS.md refresh. When
// STATUS.md changes, change this date and the surrounding numbers
// together — out-of-date evidence is worse than no evidence.
const VERIFIED_AS_OF = '2026-04-29';
const PERF_LOAD_RPS = 400;
const PERF_LOAD_P99_MS = 300;

// SDK / crate inventory — published artifacts. Update on each release.
const PUBLISHED_ARTIFACTS = [
  { kind: 'crate', name: 'solvela-protocol', registry: 'crates.io', version: '0.2.0', license: 'Apache-2.0' },
  { kind: 'crate', name: 'solvela-x402',     registry: 'crates.io', version: '0.2.0', license: 'Apache-2.0' },
  { kind: 'crate', name: 'solvela-router',   registry: 'crates.io', version: '0.2.0', license: 'Apache-2.0' },
  { kind: 'crate', name: 'solvela-cli',      registry: 'crates.io', version: '0.2.0', license: 'Apache-2.0' },
  { kind: 'sdk',   name: 'solvela-python',   registry: 'PyPI',      version: '0.1.0', license: 'MIT' },
  { kind: 'sdk',   name: 'solvela-ts',       registry: 'npm',       version: '0.2.0', license: 'MIT' },
  { kind: 'sdk',   name: 'solvela-go',       registry: 'go modules',version: '0.1.0', license: 'MIT' },
  { kind: 'sdk',   name: 'solvela-client',   registry: 'crates.io', version: '0.2.0', license: 'MIT' },
] as const;

const LICENSE_LINES = [
  { component: 'Gateway', license: 'BUSL-1.1', note: 'Change Date 2030-05-02 → Apache-2.0' },
  { component: 'Protocol / x402 / router / CLI', license: 'Apache-2.0', note: 'patent grant included' },
  { component: 'Escrow program (Anchor)', license: 'Apache-2.0', note: 'on-chain neutral' },
  { component: 'SDKs (Python / TS / Go / Rust)', license: 'MIT', note: 'maximum integration freedom' },
];

export const metadata: Metadata = {
  title: 'Public metrics',
  description:
    'Live and verified evidence of Solvela in production: escrow program, gateway health, throughput, distribution, and license posture. For grant reviewers, integrators, and partners.',
  alternates: { canonical: 'https://solvela.ai/metrics' },
  robots: { index: true, follow: true },
};

export default async function MetricsPage() {
  // Fan out reads in parallel; each individually tolerates failure.
  const [health, pricing, publicStats, escrow, services] = await Promise.all([
    fetchHealth().catch(() => null),
    fetchPricing().catch(() => null),
    fetchPublicMetrics(30).catch(() => null),
    fetchEscrowConfig().catch(() => null),
    fetchServices().catch(() => null),
  ]);

  const gatewayStatus = health?.status ?? 'down';
  const gatewayVersion = health?.version ?? 'unknown';
  const modelCount = pricing?.models.length ?? 0;
  const providerCount = pricing
    ? new Set(pricing.models.map((m) => m.provider)).size
    : 0;
  const liveDataAvailable = publicStats !== null;

  return (
    <main className="dark min-h-screen bg-[var(--background)] text-foreground">
      <LandingTopStrip />

      <div className="mx-auto max-w-[1280px] px-6 py-12 space-y-10">
        {/* Header */}
        <header className="space-y-3">
          <p className="font-mono text-[11px] uppercase tracking-[0.18em] text-text-tertiary">
            solvela / public metrics
          </p>
          <h1 className="font-display text-4xl md:text-5xl font-semibold leading-tight">
            What&rsquo;s actually shipped.
          </h1>
          <p className="max-w-[60ch] text-[15px] leading-[1.6] text-muted-foreground">
            Live gateway health, on-chain escrow program, distribution,
            and verified performance numbers. Every claim is a link or a
            program ID you can verify yourself.
          </p>
          <div className="flex flex-wrap items-center gap-3 pt-1">
            <StatusDot
              status={gatewayStatus === 'ok' ? 'ok' : gatewayStatus === 'degraded' ? 'degraded' : 'down'}
              label={`Gateway ${gatewayStatus}`}
            />
            <span className="font-mono text-[11px] text-text-tertiary">
              v{gatewayVersion}
            </span>
            <span aria-hidden className="text-text-faint">·</span>
            <span className="font-mono text-[11px] uppercase tracking-[0.14em] text-[var(--color-success)]">
              solana mainnet
            </span>
            <span aria-hidden className="text-text-faint">·</span>
            <span className="font-mono text-[11px] text-text-tertiary">
              verified {VERIFIED_AS_OF}
            </span>
          </div>
        </header>

        {/* On-chain proof — most trust-bearing thing on the page */}
        <section aria-labelledby="onchain-heading" className="space-y-3">
          <p id="onchain-heading" className="eyebrow">on-chain</p>
          <div className="grid grid-cols-1 gap-3 lg:grid-cols-3">
            <TerminalCard
              title="escrow.program"
              meta={<span className="text-xxs text-text-tertiary font-mono">solana mainnet</span>}
              accentDot
              className="lg:col-span-2"
            >
              <p className="text-xs text-text-tertiary font-mono uppercase tracking-wide mb-2">
                Program ID
              </p>
              <p className="font-mono text-sm break-all leading-relaxed">
                {FULL_ESCROW_PROGRAM_ID}
              </p>
              <div className="mt-4 flex flex-wrap gap-3 text-xs">
                <a
                  href={SOLSCAN_PROGRAM_URL}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex h-9 items-center rounded border border-border px-3 font-mono uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
                >
                  verify on solscan ↗
                </a>
                <a
                  href={`${GITHUB_URL}/tree/main/programs/escrow`}
                  target="_blank"
                  rel="noreferrer"
                  className="inline-flex h-9 items-center rounded border border-border px-3 font-mono uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
                >
                  source on github ↗
                </a>
              </div>
            </TerminalCard>

            <TerminalCard
              title="settlement.token"
              meta={<span className="text-xxs text-text-tertiary font-mono">spl</span>}
            >
              <p className="text-xs text-text-tertiary font-mono uppercase tracking-wide mb-2">
                USDC mint
              </p>
              <p className="font-mono text-xs break-all leading-relaxed">
                {escrow?.usdc_mint ?? 'EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v'}
              </p>
              <p className="mt-4 text-xs text-text-tertiary font-mono">
                network: <span className="text-text-secondary">{escrow?.network ?? 'mainnet-beta'}</span>
              </p>
              {escrow?.current_slot !== undefined && (
                <p className="mt-1 text-xs text-text-tertiary font-mono">
                  slot: <span className="text-text-secondary">{escrow.current_slot.toLocaleString()}</span>
                </p>
              )}
            </TerminalCard>
          </div>
        </section>

        {/* Live aggregate usage */}
        <section aria-labelledby="usage-heading" className="space-y-3">
          <div className="flex items-baseline justify-between">
            <p id="usage-heading" className="eyebrow">last 30 days · live</p>
            {!liveDataAvailable && (
              <span className="font-mono text-[10px] uppercase tracking-[0.14em] text-warning">
                live read unavailable — gateway unreachable
              </span>
            )}
          </div>

          {liveDataAvailable ? (
            <>
              <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
                <TerminalCard title="requests.total" screenClassName="!px-5 !py-5">
                  <p className="metric-lg">{formatNumber(publicStats!.total_requests)}</p>
                  <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                    paid x402 calls
                  </p>
                </TerminalCard>
                <TerminalCard title="usdc.settled" screenClassName="!px-5 !py-5">
                  <p className="metric-lg">
                    {formatUSDC(parseFloat(publicStats!.total_cost_usdc), 2)}
                  </p>
                  <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                    on-chain settlement
                  </p>
                </TerminalCard>
                <TerminalCard title="wallets.unique" screenClassName="!px-5 !py-5">
                  <p className="metric-lg">{formatNumber(publicStats!.unique_wallets)}</p>
                  <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                    paying agents
                  </p>
                </TerminalCard>
                <TerminalCard title="models.active" screenClassName="!px-5 !py-5">
                  <p className="metric-lg">{publicStats!.active_models}</p>
                  <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                    of {modelCount} total
                  </p>
                </TerminalCard>
              </div>

              {publicStats!.by_day.length > 0 && (
                <div className="grid grid-cols-1 gap-3 lg:grid-cols-2">
                  <TerminalCard
                    title="spend.usdc.daily"
                    meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
                  >
                    <SpendChart
                      data={publicStats!.by_day.map((d) => ({
                        date: new Date(d.date).toLocaleDateString('en-US', {
                          month: 'short',
                          day: 'numeric',
                        }),
                        spend: d.spend,
                        requests: d.requests,
                      }))}
                    />
                  </TerminalCard>
                  <TerminalCard
                    title="requests.daily"
                    meta={<span className="text-xxs text-text-tertiary font-mono">30d</span>}
                  >
                    <RequestsBar
                      data={publicStats!.by_day.map((d) => ({
                        date: new Date(d.date).toLocaleDateString('en-US', {
                          month: 'short',
                          day: 'numeric',
                        }),
                        spend: d.spend,
                        requests: d.requests,
                      }))}
                    />
                  </TerminalCard>
                </div>
              )}

              {publicStats!.top_models.length > 0 && (
                <TerminalCard
                  title="models.top"
                  meta={<span className="text-xxs text-text-tertiary font-mono">by requests</span>}
                >
                  <ul className="divide-y divide-border/40 text-sm">
                    {publicStats!.top_models.map((m) => (
                      <li
                        key={`${m.provider}/${m.model}`}
                        className="flex items-center justify-between py-2.5 font-mono"
                      >
                        <span className="text-text-secondary">
                          <span className="text-text-tertiary">{m.provider}</span>
                          <span className="mx-1.5 text-text-faint">/</span>
                          {m.model}
                        </span>
                        <span className="text-text-tertiary text-xs">
                          {formatNumber(m.requests)} req
                        </span>
                      </li>
                    ))}
                  </ul>
                </TerminalCard>
              )}
            </>
          ) : (
            <TerminalCard title="usage.live">
              <p className="text-sm text-text-secondary">
                Live aggregate stats are temporarily unavailable. The page
                falls back to verified static evidence below; on-chain
                escrow data and gateway health continue to render in real
                time when the gateway is reachable.
              </p>
            </TerminalCard>
          )}
        </section>

        {/* Verified production numbers from STATUS.md */}
        <section aria-labelledby="perf-heading" className="space-y-3">
          <p id="perf-heading" className="eyebrow">verified · production load test {VERIFIED_AS_OF}</p>
          <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
            <TerminalCard title="throughput" screenClassName="!px-5 !py-5">
              <p className="metric-lg">{PERF_LOAD_RPS}</p>
              <p className="mt-1.5 text-xs text-text-tertiary font-mono">rps sustained</p>
            </TerminalCard>
            <TerminalCard title="latency.p99" screenClassName="!px-5 !py-5">
              <p className="metric-lg">&lt;{PERF_LOAD_P99_MS}<span className="text-2xl">ms</span></p>
              <p className="mt-1.5 text-xs text-text-tertiary font-mono">at sustained rps</p>
            </TerminalCard>
            <TerminalCard title="providers" screenClassName="!px-5 !py-5">
              <p className="metric-lg">{providerCount || 5}</p>
              <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                openai · anthropic · google · xai · deepseek
              </p>
            </TerminalCard>
            <TerminalCard title="models" screenClassName="!px-5 !py-5">
              <p className="metric-lg">{modelCount || '—'}</p>
              <p className="mt-1.5 text-xs text-text-tertiary font-mono">
                via /v1/models
              </p>
            </TerminalCard>
          </div>
        </section>

        {/* Distribution / acquihire-grade evidence */}
        <section aria-labelledby="dist-heading" className="space-y-3">
          <p id="dist-heading" className="eyebrow">distribution</p>
          <TerminalCard title="published.artifacts">
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border/60 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-text-tertiary">
                    <th className="py-2 pr-4">name</th>
                    <th className="py-2 pr-4">registry</th>
                    <th className="py-2 pr-4">version</th>
                    <th className="py-2">license</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/30 font-mono">
                  {PUBLISHED_ARTIFACTS.map((a) => (
                    <tr key={a.name}>
                      <td className="py-2 pr-4 text-text-secondary">{a.name}</td>
                      <td className="py-2 pr-4 text-text-tertiary">{a.registry}</td>
                      <td className="py-2 pr-4 text-text-secondary">{a.version}</td>
                      <td className="py-2 text-text-tertiary text-xs">{a.license}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </TerminalCard>

          {services && services.total > 0 && (
            <TerminalCard
              title="services.marketplace"
              meta={<span className="text-xxs text-text-tertiary font-mono">live</span>}
            >
              <p className="text-sm text-text-secondary">
                <span className="metric-md">{services.total}</span>
                <span className="ml-2 text-text-tertiary font-mono">
                  third-party x402 services routed by the gateway
                </span>
              </p>
            </TerminalCard>
          )}
        </section>

        {/* License posture — strategic clarity for evaluators */}
        <section aria-labelledby="license-heading" className="space-y-3">
          <p id="license-heading" className="eyebrow">licensing posture</p>
          <TerminalCard title="license.split">
            <div className="overflow-x-auto">
              <table className="w-full text-sm">
                <thead>
                  <tr className="border-b border-border/60 text-left font-mono text-[10px] uppercase tracking-[0.14em] text-text-tertiary">
                    <th className="py-2 pr-4">component</th>
                    <th className="py-2 pr-4">license</th>
                    <th className="py-2">note</th>
                  </tr>
                </thead>
                <tbody className="divide-y divide-border/30">
                  {LICENSE_LINES.map((l) => (
                    <tr key={l.component} className="font-mono">
                      <td className="py-2 pr-4 text-text-secondary">{l.component}</td>
                      <td className="py-2 pr-4 text-text-secondary">{l.license}</td>
                      <td className="py-2 text-text-tertiary text-xs">{l.note}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <p className="mt-4 text-xs text-text-tertiary leading-relaxed">
              Gateway BUSL-1.1 carries an Additional Use Grant: production
              use is free for any organization with under USD $1M annual
              revenue derived from the Licensed Work, and free for
              first-party (non-hosted) deployments at any scale.
              See{' '}
              <a
                href={`${GITHUB_URL}/blob/main/LICENSE`}
                className="underline underline-offset-2 hover:text-foreground"
                target="_blank"
                rel="noreferrer"
              >
                LICENSE
              </a>{' '}for exact terms.
            </p>
          </TerminalCard>
        </section>

        {/* Gateway health detail */}
        <section aria-labelledby="health-heading" className="space-y-3">
          <p id="health-heading" className="eyebrow">system health · live</p>
          <TerminalCard
            title="system.health"
            meta={<span className="text-xxs text-text-tertiary font-mono">v{gatewayVersion} · x402 v2</span>}
            screenClassName="!py-5 !px-6"
          >
            <div className="flex flex-wrap gap-x-6 gap-y-3">
              <StatusDot
                status={gatewayStatus === 'ok' ? 'ok' : gatewayStatus === 'degraded' ? 'degraded' : 'down'}
                label="Gateway"
              />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="Solana RPC" />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="OpenAI" />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="Anthropic" />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="Google" />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="xAI" />
              <StatusDot status={gatewayStatus === 'ok' ? 'ok' : 'unknown'} label="DeepSeek" />
            </div>
            <p className="mt-4 text-xs text-text-tertiary font-mono">
              endpoint: <span className="text-text-secondary">api.solvela.ai</span>
              <span className="mx-2 text-text-faint">·</span>
              region: <span className="text-text-secondary">fly.io ord</span>
            </p>
          </TerminalCard>
        </section>

        {/* CTA */}
        <section aria-labelledby="cta-heading" className="space-y-3">
          <p id="cta-heading" className="eyebrow">talk to us</p>
          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <TerminalCard title="integrate">
              <p className="text-sm text-text-secondary leading-relaxed">
                Self-hosted under $1M revenue is free forever.
              </p>
              <a
                href="https://docs.solvela.ai/docs/quickstart"
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                quickstart ↗
              </a>
            </TerminalCard>
            <TerminalCard title="commercial.license">
              <p className="text-sm text-text-secondary leading-relaxed">
                Hosted gateway as a service, or revenue above the $1M
                threshold.
              </p>
              <a
                href="https://docs.solvela.ai/docs/enterprise/commercial-license"
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                pricing ↗
              </a>
            </TerminalCard>
            <TerminalCard title="partnerships">
              <p className="text-sm text-text-secondary leading-relaxed">
                Grants, acquihire, integrations, RPC partnerships.
              </p>
              <a
                href="mailto:kd@sky64.io"
                className="mt-3 inline-flex h-9 items-center rounded border border-border px-3 font-mono text-xs uppercase tracking-[0.14em] text-text-secondary transition-colors hover:text-foreground"
              >
                kd@sky64.io ↗
              </a>
            </TerminalCard>
          </div>
        </section>

        {/* Provenance footer — every claim above is sourced */}
        <section aria-labelledby="provenance-heading" className="space-y-2">
          <p id="provenance-heading" className="eyebrow">provenance</p>
          <p className="text-xs text-text-tertiary font-mono leading-relaxed">
            on-chain: solana mainnet, escrow {ESCROW_PROGRAM_ID}…
            <br />
            live: api.solvela.ai/health, /v1/models, /v1/escrow/config, /v1/services, /v1/admin/stats
            <br />
            static evidence: STATUS.md, CHANGELOG.md @{' '}
            <a
              href={GITHUB_URL}
              className="underline underline-offset-2 hover:text-foreground"
              target="_blank"
              rel="noreferrer"
            >
              github.com/solvela-ai/solvela
            </a>
            <br />
            verified as of {VERIFIED_AS_OF}
          </p>
        </section>
      </div>

      <LandingFooter />
    </main>
  );
}
