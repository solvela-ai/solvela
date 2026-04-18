'use client'

import { useEffect, useRef, useState, type ReactNode } from 'react'
import { cn } from '@/lib/utils'

interface InViewProps {
  children: ReactNode
  className?: string
  activeClassName?: string
  threshold?: number
  rootMargin?: string
}

// Adds `activeClassName` the first time the container crosses `threshold`.
// Respects prefers-reduced-motion (applies the class immediately).
export function InView({
  children,
  className,
  activeClassName = 'is-visible',
  threshold = 0.3,
  rootMargin = '0px 0px -8% 0px',
}: InViewProps) {
  const ref = useRef<HTMLDivElement>(null)
  const [active, setActive] = useState(false)

  useEffect(() => {
    const node = ref.current
    if (!node) return
    const reduced = window.matchMedia('(prefers-reduced-motion: reduce)').matches
    if (reduced) {
      requestAnimationFrame(() => setActive(true))
      return
    }
    const io = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            setActive(true)
            io.disconnect()
            break
          }
        }
      },
      { threshold, rootMargin }
    )
    io.observe(node)
    return () => io.disconnect()
  }, [threshold, rootMargin])

  return (
    <div ref={ref} className={cn(className, active && activeClassName)}>
      {children}
    </div>
  )
}
