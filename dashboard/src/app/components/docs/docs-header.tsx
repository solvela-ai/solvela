'use client'

import { useState, useCallback } from 'react'
import Link from 'next/link'
import { SearchTrigger } from './search-dialog'
import { ThemeToggle } from './theme-toggle'
import { MobileSidebar } from './mobile-sidebar'
import { DocsNavTabs } from './docs-nav-tabs'
import { siteConfig } from '@/lib/theme-config'
import { Icon } from '@/lib/icons'
import type { Root } from 'fumadocs-core/page-tree'

interface DocsHeaderProps {
  tree: Root
}

export function DocsHeader({ tree }: DocsHeaderProps) {
  const [isMobileMenuOpen, setIsMobileMenuOpen] = useState(false)

  const openMobileMenu = useCallback(() => setIsMobileMenuOpen(true), [])
  const closeMobileMenu = useCallback(() => setIsMobileMenuOpen(false), [])

  return (
    <>
      <header className="sticky top-0 z-50 w-full border-b border-border bg-[var(--sidebar-bg)]">
        <div className="px-4 sm:px-6 lg:px-8">
          <div className="flex h-14 items-center gap-6">
            {/* Left: Hamburger + Logo + Tabs (desktop) */}
            <div className="flex items-center gap-5 shrink-0">
              {/* Mobile menu button */}
              <button
                onClick={openMobileMenu}
                className="lg:hidden p-2 -ml-2 text-muted-foreground hover:text-foreground rounded-md transition-colors min-w-[44px] min-h-[44px] flex items-center justify-center"
                aria-label="Open menu"
                aria-expanded={isMobileMenuOpen}
              >
                <Icon name="menu" className="w-5 h-5" />
              </button>

              {/* Logo */}
              <Link href="/" className="flex items-center gap-2.5">
                <span className="text-[15px] font-semibold text-[var(--heading-color)] tracking-tight">{siteConfig.name}</span>
                <span className="text-[11px] font-medium text-muted-foreground tracking-wide uppercase">Docs</span>
              </Link>

              {/* Nav tabs — desktop only, inline with header */}
              <div className="hidden lg:block ml-2">
                <DocsNavTabs />
              </div>
            </div>

            {/* Center spacer */}
            <div className="flex-1" />

            {/* Right: Search + Links */}
            <div className="flex items-center gap-2 shrink-0">
              <div className="hidden sm:block w-56 lg:w-72">
                <SearchTrigger />
              </div>
              {siteConfig.links.github && (
                <a
                  href={siteConfig.links.github}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="p-2 text-muted-foreground hover:text-foreground transition-colors"
                  aria-label="GitHub"
                >
                  <Icon name="github" className="w-[18px] h-[18px]" />
                </a>
              )}
              <ThemeToggle />
            </div>
          </div>
        </div>
      </header>

      {/* Mobile search — below header on sm and below */}
      <div className="sm:hidden sticky top-14 z-40 bg-[var(--sidebar-bg)] border-b border-border px-4 py-2">
        <SearchTrigger />
      </div>

      {/* Mobile sidebar */}
      <MobileSidebar
        tree={tree}
        isOpen={isMobileMenuOpen}
        onClose={closeMobileMenu}
      />
    </>
  )
}
