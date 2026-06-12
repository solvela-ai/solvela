import type { Metadata } from 'next'
import { LandingTopStrip } from '@/components/landing/landing-chrome'
import { SmoothScroll } from '@/components/motion/smooth-scroll'
import { HeroPanel } from '@/components/landing/hero-panel'
import { A2ASection } from '@/components/landing/a2a-section'
import { ArchitectureSection } from '@/components/landing/architecture-section'
import { LifecycleScene } from '@/components/landing/lifecycle-scene'
import { WhySolvela } from '@/components/landing/why-solvela'
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
    // No `images` key here on purpose: the file-convention
    // src/app/opengraph-image.tsx supplies the dynamic OG image and only
    // takes precedence when metadata doesn't hardcode one.
    title: 'Solvela — trustless escrow for agent payments',
    description:
      'Solana-native x402 gateway for AI agents. Pay only for what your agent receives — escrow-settled in USDC-SPL.',
    url: 'https://solvela.ai',
    siteName: 'Solvela',
    type: 'website',
  },
  twitter: {
    // Same rationale: omit `images` so the dynamic opengraph-image is used.
    card: 'summary_large_image',
    title: 'Solvela — trustless escrow for agent payments',
    description:
      'Solana-native x402 gateway for AI agents. Pay only for what your agent receives.',
  },
  alternates: { canonical: 'https://solvela.ai' },
}

// Structured data — Organization + SoftwareApplication. Emitted as a single
// @graph so crawlers read both entities from one script. No ratings, reviews,
// prices, or other fields we can't substantiate.
const JSON_LD = {
  '@context': 'https://schema.org',
  '@graph': [
    {
      '@type': 'Organization',
      '@id': 'https://solvela.ai/#organization',
      name: 'Solvela',
      url: 'https://solvela.ai',
      sameAs: ['https://github.com/solvela-ai/solvela'],
      contactPoint: {
        '@type': 'ContactPoint',
        email: 'partnerships@solvela.ai',
        contactType: 'partnerships',
      },
    },
    {
      '@type': 'SoftwareApplication',
      '@id': 'https://solvela.ai/#software',
      name: 'Solvela',
      url: 'https://solvela.ai',
      applicationCategory: 'DeveloperApplication',
      operatingSystem: 'any',
      description:
        'Solana-native x402 gateway for AI agents. Pay only for what your agent receives — escrow-settled in USDC-SPL.',
      publisher: { '@id': 'https://solvela.ai/#organization' },
    },
  ],
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
      <script
        type="application/ld+json"
        // Serialized server-side; content is static and contains no user input.
        dangerouslySetInnerHTML={{ __html: JSON.stringify(JSON_LD) }}
      />
      <SmoothScroll>
        <LandingTopStrip />
        <HeroPanel />
        <A2ASection />
        <LifecycleScene />
        <WhySolvela />
        <ProviderRow />
        <EnterprisePanel />
        <ArchitectureSection />
        <SdkCtaPanel preHighlighted={preHighlighted} />
        <LandingFooter />
      </SmoothScroll>
    </main>
  )
}
