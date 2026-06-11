'use client'

import { ReactLenis } from 'lenis/react'
import { useReducedMotion } from 'motion/react'

/**
 * Lenis smooth scrolling, scoped to the marketing surface only — docs and
 * dashboard keep native scrolling. Disabled entirely under
 * prefers-reduced-motion (Lenis is never mounted, not just paused).
 */
export function SmoothScroll({ children }: { children: React.ReactNode }) {
  const reduced = useReducedMotion()
  if (reduced) return <>{children}</>
  return (
    <ReactLenis root options={{ lerp: 0.12, wheelMultiplier: 1, touchMultiplier: 1.4 }}>
      {children}
    </ReactLenis>
  )
}
