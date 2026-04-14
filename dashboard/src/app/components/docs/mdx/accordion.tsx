'use client'

import { useState, useId } from 'react'
import { cn } from '@/lib/utils'

interface AccordionGroupProps {
  children: React.ReactNode
}

export function AccordionGroup({ children }: AccordionGroupProps) {
  return (
    <div className="my-6 divide-y divide-border rounded-lg border border-border">
      {children}
    </div>
  )
}

interface AccordionProps {
  title: string
  children: React.ReactNode
  defaultOpen?: boolean
}

export function Accordion({ title, children, defaultOpen = false }: AccordionProps) {
  const [isOpen, setIsOpen] = useState(defaultOpen)
  const contentId = useId()

  return (
    <div>
      <button
        onClick={() => setIsOpen(!isOpen)}
        aria-expanded={isOpen}
        aria-controls={contentId}
        className="flex w-full items-center justify-between px-4 py-4 text-left font-medium text-foreground hover:bg-[var(--card)] transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-foreground focus-visible:ring-inset"
      >
        <span>{title}</span>
        <svg
          aria-hidden="true"
          className={cn(
            'w-5 h-5 text-muted-foreground transition-transform duration-200',
            isOpen && 'rotate-180'
          )}
          fill="none"
          viewBox="0 0 24 24"
          stroke="currentColor"
          strokeWidth={2}
        >
          <path strokeLinecap="round" strokeLinejoin="round" d="M19 9l-7 7-7-7" />
        </svg>
      </button>
      <div
        id={contentId}
        className={cn(
          'grid transition-[grid-template-rows] duration-200 ease-out',
          isOpen ? 'grid-rows-[1fr]' : 'grid-rows-[0fr]'
        )}
      >
        <div className="overflow-hidden">
          <div className="px-4 pb-4 text-muted-foreground [&>p]:m-0">
            {children}
          </div>
        </div>
      </div>
    </div>
  )
}
