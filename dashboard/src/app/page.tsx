import type { Metadata } from 'next'
import { LandingTopStrip, LandingTicker } from '@/components/landing/landing-chrome'
import { HeroPanel } from '@/components/landing/hero-panel'
import { PartnersRow } from '@/components/landing/partners-row'
import { EscrowPanel } from '@/components/landing/escrow-panel'
import { ProviderRow } from '@/components/landing/provider-row'
import { EnterprisePanel } from '@/components/landing/enterprise-panel'
import { SdkCtaPanel } from '@/components/landing/sdk-cta-panel'
import { LandingFooter } from '@/components/landing/landing-footer'

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

export default function LandingPage() {
  return (
    <main className="min-h-screen bg-[var(--background)] text-foreground">
      <LandingTopStrip />
      <HeroPanel />
      <LandingTicker />
      <PartnersRow />
      <EscrowPanel />
      <ProviderRow />
      <EnterprisePanel />
      <SdkCtaPanel />
      <LandingFooter />
    </main>
  )
}
