'use client'

import { useState } from 'react'
import { Check, Copy } from 'lucide-react'
import { cn } from '@/lib/utils'

interface CopyButtonProps {
  text: string
  label?: string
  className?: string
}

export function CopyButton({ text, label = 'copy', className }: CopyButtonProps) {
  const [copied, setCopied] = useState(false)
  const [failed, setFailed] = useState(false)

  function handleCopy() {
    navigator.clipboard.writeText(text).then(
      () => {
        setCopied(true)
        window.setTimeout(() => setCopied(false), 1400)
      },
      () => {
        // Clipboard blocked (insecure origin, permissions, etc.) — surface briefly.
        setFailed(true)
        window.setTimeout(() => setFailed(false), 1400)
      }
    )
  }

  return (
    <button
      type="button"
      onClick={handleCopy}
      aria-label={copied ? 'Copied' : failed ? 'Copy failed' : 'Copy to clipboard'}
      className={cn(
        'inline-flex items-center gap-1.5 h-11 min-w-11 px-3 py-1 rounded-md border border-border text-text-tertiary hover:text-foreground hover:border-[var(--accent-salmon)] transition-colors font-mono text-[11px] uppercase tracking-[0.08em]',
        className
      )}
    >
      {copied ? <Check className="w-3 h-3" /> : <Copy className="w-3 h-3" />}
      <span>{copied ? 'copied' : failed ? 'failed' : label}</span>
    </button>
  )
}
