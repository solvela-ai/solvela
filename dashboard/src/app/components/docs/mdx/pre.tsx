'use client'

import { useState, useRef } from 'react'
import { cn } from '@/lib/utils'
import { Icon } from '@/lib/icons'

interface PreProps extends React.HTMLAttributes<HTMLPreElement> {
  children: React.ReactNode
  'data-language'?: string
}

export function Pre({ children, className, 'data-language': language, ...props }: PreProps) {
  const [copied, setCopied] = useState(false)
  const preRef = useRef<HTMLPreElement>(null)

  const handleCopy = async () => {
    const code = preRef.current?.textContent
    if (code) {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 2000)
    }
  }

  return (
    <div className="group relative my-6">
      {/* Container with border and rounded corners */}
      <div className="relative rounded-lg border border-border bg-[var(--card)] overflow-hidden">
        {/* Code content */}
        <pre
          ref={preRef}
          className={cn(
            'overflow-x-auto p-4 text-sm leading-relaxed',
            className
          )}
          {...props}
        >
          {children}
        </pre>

        {/* Copy button - positioned in top right */}
        <button
          onClick={handleCopy}
          className={cn(
            'absolute top-2 right-2 flex items-center gap-1.5 px-2 py-1 rounded-md text-xs font-medium transition-all',
            'text-muted-foreground hover:text-foreground',
            'bg-background/80 hover:bg-background border border-border/50',
            'opacity-60 hover:opacity-100',
            copied && 'opacity-100 text-[var(--callout-success)]'
          )}
        >
          {copied ? (
            <>
              <Icon name="check" className="w-3.5 h-3.5" />
              Copied
            </>
          ) : (
            <>
              <Icon name="copy" className="w-3.5 h-3.5" />
              Copy
            </>
          )}
        </button>
      </div>
    </div>
  )
}
