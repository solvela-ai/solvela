import type { Metadata } from 'next'
import { LandingTopStrip, LandingTicker } from '@/components/landing/landing-chrome'
import { HeroPanel } from '@/components/landing/hero-panel'
import { EscrowPanel } from '@/components/landing/escrow-panel'
import { ProviderRow } from '@/components/landing/provider-row'
import { EnterprisePanel } from '@/components/landing/enterprise-panel'
import { SdkCtaPanel } from '@/components/landing/sdk-cta-panel'
import { LandingFooter } from '@/components/landing/landing-footer'
import { SAMPLES } from '@/components/landing/sdk-samples'
import { highlight } from '@/lib/shiki/highlighter'

export const metadata: Metadata = {
  title: 'Solvela — trustless escrow for agent payments',
  description:
    'Solana-native x402 gateway for AI agents. Pay only for what your agent receives — escrow-settled in USDC-SPL. No accounts, no API keys, just wallets.',
  openGraph: {
    title: 'Solvela — trustless escrow for agent payments',
    description:
      'Solana-native x402 gateway for AI agents. Pay only for what your agent receives — escrow-settled in USDC-SPL.',
    url: 'https://solvela.ai',
    siteName: 'Solvela',
    images: [{ url: 'https://solvela.ai/logo.png', width: 1200, height: 630 }],
    type: 'website',
  },
  twitter: {
    card: 'summary_large_image',
    title: 'Solvela — trustless escrow for agent payments',
    description:
      'Solana-native x402 gateway for AI agents. Pay only for what your agent receives.',
    images: ['https://solvela.ai/logo.png'],
  },
  alternates: { canonical: 'https://solvela.ai' },
}

export default async function LandingPage() {
  const entries = await Promise.all(
    SAMPLES.map(async (s) => [s.id, await highlight(s.code, s.lang)] as const),
  )
  const preHighlighted = Object.fromEntries(entries)

  return (
    // `dark` class scopes the brand palette to this subtree regardless of the
    // ambient next-themes state. Landing is a dark-first marketing surface;
    // forcing it here prevents OS-light users from falling through to the
    // (untested for landing) light-mode token set.
    <main className="dark min-h-screen bg-[var(--background)] text-foreground">
      <LandingTopStrip />
      <HeroPanel />
      <LandingTicker />
      <EscrowPanel />
      <ProviderRow />
      <EnterprisePanel />
      <SdkCtaPanel preHighlighted={preHighlighted} />
      <LandingFooter />
    </main>
  )
}
