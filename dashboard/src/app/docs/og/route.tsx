/**
 * Dynamic OG image for documentation pages.
 *
 * Why a route handler instead of the `opengraph-image` file convention:
 * Next.js forbids metadata image files under an optional catch-all segment
 * (`/docs/[[...slug]]/opengraph-image` → "Optional catch-all must be the last
 * part of the URL"). So the docs page's generateMetadata points
 * openGraph.images at this handler with a `?slug=` query, and we resolve the
 * page title from the same fumadocs source the page uses.
 *
 * No external network fetch at request time; see src/lib/og.tsx.
 */
import type { NextRequest } from 'next/server'
import { source } from '@/lib/source'
import { renderOgImage } from '@/lib/og'

export const runtime = 'nodejs'

export function GET(req: NextRequest) {
  const raw = req.nextUrl.searchParams.get('slug') ?? ''
  // slug is the page url path under /docs, slash-joined (e.g. "concepts/x402").
  const slug = raw ? raw.split('/').filter(Boolean) : undefined
  const page = source.getPage(slug)
  const title = page?.data.title ?? 'Documentation'

  return renderOgImage({
    eyebrow: 'solvela docs',
    kicker: 'documentation',
    title,
  })
}
