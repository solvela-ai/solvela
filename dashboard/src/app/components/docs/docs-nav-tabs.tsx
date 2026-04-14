'use client'

import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { cn } from '@/lib/utils'

interface NavTab {
  label: string
  href: string
  matchPrefix: string
}

interface DocsNavTabsProps {
  tabs?: NavTab[]
}

const defaultTabs: NavTab[] = [
  { label: 'Documentation', href: '/docs', matchPrefix: '/docs' },
  { label: 'API Reference', href: '/docs/api-reference', matchPrefix: '/docs/api-reference' },
]

export function DocsNavTabs({ tabs = defaultTabs }: DocsNavTabsProps) {
  const pathname = usePathname()

  const activeTab = tabs
    .filter(tab => pathname.startsWith(tab.matchPrefix))
    .sort((a, b) => b.matchPrefix.length - a.matchPrefix.length)[0]

  if (tabs.length <= 1) {
    return null
  }

  return (
    <nav className="flex items-center gap-1" aria-label="Documentation sections">
      {tabs.map((tab) => {
        const isActive = activeTab?.matchPrefix === tab.matchPrefix

        return (
          <Link
            key={tab.href}
            href={tab.href}
            className={cn(
              'px-3 py-1.5 text-[13px] font-medium rounded-md transition-colors',
              isActive
                ? 'text-[var(--heading-color)] bg-[var(--card)]'
                : 'text-muted-foreground hover:text-foreground hover:bg-[var(--card)]/50'
            )}
          >
            {tab.label}
          </Link>
        )
      })}
    </nav>
  )
}
