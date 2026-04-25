// Server-only Shiki highlighter for landing-page code samples.
// Uses the same solvela-dark/solvela-light themes that Fumadocs applies
// to the docs site, so code coloring is consistent across both surfaces.
//
// Output is stripped of Shiki's outer <pre> wrapper so the caller can
// place the inner <code> inside its own styled <pre>.

// This module is intended for server-only use (imported from async
// Server Components and build-time routines). It pulls in the full Shiki
// WASM engine — do not import it from any `'use client'` boundary.
import { getSingletonHighlighter, type Highlighter } from 'shiki'
import solvelaDark from './solvela-dark.json' with { type: 'json' }
import solvelaLight from './solvela-light.json' with { type: 'json' }

type ThemeInput = Parameters<typeof getSingletonHighlighter>[0] extends {
  themes?: infer T
}
  ? T extends (infer U)[]
    ? U
    : never
  : never

const SUPPORTED_LANGS = [
  'typescript',
  'tsx',
  'javascript',
  'python',
  'go',
  'bash',
  'shell',
  'json',
] as const

export type SupportedLang = (typeof SUPPORTED_LANGS)[number]

let singleton: Promise<Highlighter> | null = null

function load() {
  if (!singleton) {
    singleton = getSingletonHighlighter({
      themes: [solvelaDark as ThemeInput, solvelaLight as ThemeInput],
      langs: [...SUPPORTED_LANGS],
    })
  }
  return singleton
}

export async function highlight(code: string, lang: SupportedLang): Promise<string> {
  const h = await load()
  const html = h.codeToHtml(code, {
    lang,
    themes: { light: 'solvela-light', dark: 'solvela-dark' },
    defaultColor: false,
  })
  // Drop Shiki's outer <pre …>…</pre>; the caller owns <pre> styling.
  return html.replace(/^<pre[^>]*>/, '').replace(/<\/pre>\s*$/, '')
}
