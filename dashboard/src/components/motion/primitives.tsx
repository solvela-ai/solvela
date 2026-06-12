'use client'

import { motion, useReducedMotion, type Variants } from 'motion/react'

/**
 * Shared motion primitives for the marketing surface. Every primitive is
 * gated on prefers-reduced-motion: reduced users get the final state with
 * no transform animation (opacity-only or none at all).
 */

const EASE_OUT = [0.16, 1, 0.3, 1] as const

export const revealVariants: Variants = {
  hidden: { opacity: 0, y: 28 },
  visible: (i: number = 0) => ({
    opacity: 1,
    y: 0,
    transition: { duration: 0.7, ease: EASE_OUT, delay: i * 0.08 },
  }),
}

/** Fade+rise on scroll into view. `index` staggers siblings. */
export function Reveal({
  children,
  index = 0,
  className,
  as: Tag = 'div',
}: {
  children: React.ReactNode
  index?: number
  className?: string
  as?: 'div' | 'section' | 'li' | 'span'
}) {
  const reduced = useReducedMotion()
  const M = motion[Tag]
  if (reduced) return <Tag className={className}>{children}</Tag>
  return (
    <M
      className={className}
      custom={index}
      initial="hidden"
      whileInView="visible"
      viewport={{ once: true, margin: '0px 0px -10% 0px' }}
      variants={revealVariants}
    >
      {children}
    </M>
  )
}

/**
 * Kinetic headline: words rise in with a slight optical overshoot.
 * Splits on spaces only (no per-character splitting — stays readable
 * for screen readers via aria-label on the wrapper).
 */
export function KineticHeadline({
  text,
  className,
}: {
  text: string
  className?: string
}) {
  const reduced = useReducedMotion()
  if (reduced) {
    return <span className={className}>{text}</span>
  }
  const words = text.split(' ')
  return (
    <span className={className} aria-label={text} role="text">
      {words.map((word, i) => (
        <span key={i} className="inline-block overflow-hidden align-bottom">
          <motion.span
            className="inline-block will-change-transform"
            aria-hidden="true"
            initial={{ y: '110%', rotate: 2.5 }}
            whileInView={{ y: '0%', rotate: 0 }}
            viewport={{ once: true }}
            transition={{ duration: 0.85, ease: EASE_OUT, delay: 0.06 * i }}
          >
            {word}
            {i < words.length - 1 ? ' ' : ''}
          </motion.span>
        </span>
      ))}
    </span>
  )
}
