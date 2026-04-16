'use client'

import { useState } from 'react'
import Link from 'next/link'
import { cn } from '@/lib/utils'

const CODE_TABS = [
  {
    label: 'curl',
    code: `curl -X POST https://solvela.ai/v1/chat/completions \\
  -H "Content-Type: application/json" \\
  -H "PAYMENT-SIGNATURE: <base64-signed-tx>" \\
  -d '{
    "model": "auto",
    "messages": [{"role": "user", "content": "Hello"}]
  }'`,
  },
  {
    label: 'TypeScript',
    code: `import { Solvela } from 'solvela'

const client = new Solvela({ wallet })
const res = await client.chat('auto', 'Hello')`,
  },
  {
    label: 'Python',
    code: `from solvela import Solvela

client = Solvela(wallet=wallet)
res = client.chat("auto", "Hello")`,
  },
  {
    label: 'Rust',
    code: `let client = SolvelaClient::new(wallet);
let res = client.chat("auto", "Hello").await?;`,
  },
]

export function HeroSplit() {
  const [activeTab, setActiveTab] = useState(0)
  const [copied, setCopied] = useState(false)

  const handleCopy = async () => {
    await navigator.clipboard.writeText(CODE_TABS[activeTab].code)
    setCopied(true)
    setTimeout(() => setCopied(false), 2000)
  }

  return (
    <div className="grid grid-cols-1 lg:grid-cols-[1fr_1.1fr] gap-10 lg:gap-14 not-prose mb-16 mt-2">
      {/* Left column */}
      <div className="flex flex-col justify-center gap-7">
        <p className="font-serif text-[1.25rem] text-[var(--foreground)]/75 leading-[1.55] max-w-[30rem]">
          A Solana-native payment gateway for AI agents. Pay for LLM API calls with USDC-SPL — no API keys, no accounts, just wallets.
        </p>
        <div className="flex flex-wrap gap-2.5">
          <Link
            href="/docs/quickstart"
            className="inline-flex items-center gap-2 px-4 py-2.5 rounded-md bg-[#1F1E1D] text-[#FAF9F5] text-sm font-medium font-sans border border-[#1F1E1D] hover:bg-[#141413] transition-colors"
          >
            Quickstart <span aria-hidden="true">→</span>
          </Link>
          <Link
            href="/docs/api"
            className="inline-flex items-center gap-2 px-4 py-2.5 rounded-md border border-[var(--border)] bg-transparent text-[var(--foreground)] text-sm font-medium font-sans hover:text-[var(--heading-color)] hover:bg-[var(--card)] transition-colors"
          >
            API Reference <span aria-hidden="true">→</span>
          </Link>
          <a
            href="https://solvela.ai"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 px-4 py-2.5 rounded-md border border-[var(--border)] bg-transparent text-[var(--foreground)] text-sm font-medium font-sans hover:text-[var(--heading-color)] hover:bg-[var(--card)] transition-colors"
          >
            Dashboard <span aria-hidden="true">↗</span>
          </a>
        </div>
      </div>

      {/* Right column — terminal-window code block */}
      <div className="terminal-card overflow-hidden">
        {/* Title bar with terminal dots + tabs */}
        <div
          role="tablist"
          aria-label="Code examples"
          className="flex items-stretch border-b border-[var(--border)] bg-[var(--popover)]"
        >
          <div className="flex items-center gap-1.5 px-4 py-2.5 border-r border-[var(--border)]">
            <span className="w-2 h-2 rounded-full bg-[var(--accent-salmon)]" aria-hidden="true" />
            <span className="w-2 h-2 rounded-full bg-[var(--border)]" aria-hidden="true" />
            <span className="w-2 h-2 rounded-full bg-[var(--border)]" aria-hidden="true" />
          </div>
          {CODE_TABS.map((tab, i) => (
            <button
              key={tab.label}
              role="tab"
              aria-selected={activeTab === i}
              aria-controls={`hero-panel-${i}`}
              id={`hero-tab-${i}`}
              tabIndex={activeTab === i ? 0 : -1}
              onClick={() => setActiveTab(i)}
              onKeyDown={(e) => {
                if (e.key === 'ArrowRight') setActiveTab((activeTab + 1) % CODE_TABS.length)
                if (e.key === 'ArrowLeft') setActiveTab((activeTab - 1 + CODE_TABS.length) % CODE_TABS.length)
              }}
              className={cn(
                'px-3.5 py-2.5 text-[11px] font-mono uppercase tracking-wider transition-colors border-b-2 -mb-px',
                'focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-[var(--accent-salmon)]',
                activeTab === i
                  ? 'border-[var(--accent-salmon)] text-[var(--heading-color)]'
                  : 'border-transparent text-[var(--muted-foreground)] hover:text-[var(--foreground)]'
              )}
            >
              {tab.label}
            </button>
          ))}
          <div className="flex-1 flex items-center justify-end px-3">
            <button
              onClick={handleCopy}
              aria-label="Copy code"
              className="text-[11px] font-mono uppercase tracking-wider text-[var(--muted-foreground)] hover:text-[var(--foreground)] transition-colors"
            >
              {copied ? 'Copied' : 'Copy'}
            </button>
          </div>
        </div>

        {/* Screen — grid background */}
        {CODE_TABS.map((tab, i) => (
          <div
            key={tab.label}
            role="tabpanel"
            id={`hero-panel-${i}`}
            aria-labelledby={`hero-tab-${i}`}
            hidden={activeTab !== i}
            className="bg-grid-dense"
          >
            <pre
              className="!border-0 !rounded-none !m-0 overflow-x-auto before:hidden !bg-transparent"
            >
              <code
                className="font-mono text-[13px] text-[var(--foreground)]/90 leading-relaxed !bg-transparent"
              >
                {tab.code}
              </code>
            </pre>
          </div>
        ))}
      </div>
    </div>
  )
}
