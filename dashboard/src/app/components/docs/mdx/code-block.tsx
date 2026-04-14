'use client'

import { useState, useRef } from 'react'
import { cn } from '@/lib/utils'

interface CodeBlockProps {
  children: React.ReactNode
  title?: string
  className?: string
}

export function CodeBlock({ children, title, className }: CodeBlockProps) {
  const [copied, setCopied] = useState(false)
  const contentRef = useRef<HTMLDivElement>(null)

  const handleCopy = async () => {
    const code = contentRef.current?.textContent
    if (code) {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }

  return (
    <div className={cn('my-6 rounded-lg overflow-hidden border border-border', className)}>
      {title && (
        <div className="flex items-center justify-between px-4 py-2 bg-[var(--sidebar-bg)] border-b border-border">
          <span className="text-xs font-mono font-medium text-muted-foreground uppercase tracking-wider">{title}</span>
          <button
            onClick={handleCopy}
            className="text-xs text-muted-foreground hover:text-foreground transition-colors"
          >
            {copied ? 'Copied!' : 'Copy'}
          </button>
        </div>
      )}
      <div ref={contentRef} className="code-block-content bg-[var(--card)] overflow-x-auto">
        {children}
      </div>
    </div>
  )
}
