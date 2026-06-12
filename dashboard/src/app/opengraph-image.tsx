import { renderOgImage, OG_SIZE, OG_CONTENT_TYPE } from '@/lib/og'

export const runtime = 'nodejs'
export const alt = 'Solvela — trustless escrow for agent payments'
export const size = OG_SIZE
export const contentType = OG_CONTENT_TYPE

export default function Image() {
  return renderOgImage({
    eyebrow: 'solana-native x402 gateway',
    title: 'Trustless escrow for agent payments',
  })
}
