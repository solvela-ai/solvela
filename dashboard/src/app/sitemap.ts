import type { MetadataRoute } from 'next'

const BASE = 'https://solvela.ai'
const DOCS = 'https://docs.solvela.ai'

export default function sitemap(): MetadataRoute.Sitemap {
  const now = new Date()
  return [
    { url: `${BASE}/`, lastModified: now, changeFrequency: 'weekly', priority: 1 },
    { url: `${DOCS}/docs`, lastModified: now, changeFrequency: 'weekly', priority: 0.9 },
    { url: `${DOCS}/docs/quickstart`, lastModified: now, priority: 0.8 },
    { url: `${DOCS}/docs/concepts/x402`, lastModified: now, priority: 0.7 },
    { url: `${DOCS}/docs/concepts/escrow`, lastModified: now, priority: 0.7 },
    { url: `${DOCS}/docs/concepts/smart-router`, lastModified: now, priority: 0.7 },
    { url: `${DOCS}/docs/api`, lastModified: now, priority: 0.7 },
    { url: `${DOCS}/docs/sdks`, lastModified: now, priority: 0.6 },
  ]
}
