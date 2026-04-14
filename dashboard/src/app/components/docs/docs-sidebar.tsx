'use client'

import { useEffect, useState } from 'react'
import Link from 'next/link'
import { usePathname } from 'next/navigation'
import { cn } from '@/lib/utils'
import { siteConfig } from '@/lib/theme-config'
import { Icon } from '@/lib/icons'
import type { Root, Node } from 'fumadocs-core/page-tree'

// HTTP method detection from page names
const HTTP_METHOD_PATTERNS: Record<string, string[]> = {
  GET: ['List', 'Get', 'Fetch', 'Read', 'Search', 'Query'],
  POST: ['Create', 'Add', 'Submit', 'Post'],
  PATCH: ['Update', 'Modify', 'Edit'],
  PUT: ['Replace', 'Set', 'Put'],
  DELETE: ['Delete', 'Remove', 'Destroy'],
  HEAD: ['Check', 'Verify', 'Exists'],
}

function getHttpMethod(name: string): string | null {
  for (const [method, patterns] of Object.entries(HTTP_METHOD_PATTERNS)) {
    if (patterns.some(pattern => name.startsWith(pattern))) {
      return method
    }
  }
  return null
}

const METHOD_COLORS: Record<string, { bg: string; text: string }> = {
  GET: { bg: 'bg-[var(--http-get)]/20', text: 'text-[var(--http-get)]' },
  POST: { bg: 'bg-[var(--http-post)]/20', text: 'text-[var(--http-post)]' },
  PATCH: { bg: 'bg-[var(--http-patch)]/20', text: 'text-[var(--http-patch)]' },
  PUT: { bg: 'bg-[var(--http-put)]/20', text: 'text-[var(--http-put)]' },
  DELETE: { bg: 'bg-[var(--http-delete)]/20', text: 'text-[var(--http-delete)]' },
  HEAD: { bg: 'bg-[var(--http-head)]/20', text: 'text-[var(--http-head)]' },
}

function HttpMethodBadge({ method }: { method: string }) {
  const colors = METHOD_COLORS[method] || { bg: 'bg-[var(--muted)]/50', text: 'text-muted-foreground' }
  const displayMethod = method === 'DELETE' ? 'DEL' : method

  return (
    <span className={cn(
      'shrink-0 px-1.5 py-0.5 text-[10px] font-semibold rounded',
      colors.bg,
      colors.text
    )}>
      {displayMethod}
    </span>
  )
}

// Group contiguous nodes under their preceding separator into collapsible sections
type Group = { label: string | null; nodes: Node[] }

function groupNodes(nodes: Node[]): Group[] {
  const groups: Group[] = [{ label: null, nodes: [] }]
  for (const node of nodes) {
    if (node.type === 'separator') {
      const label = typeof node.name === 'string' ? node.name : ''
      groups.push({ label, nodes: [] })
    } else {
      groups[groups.length - 1].nodes.push(node)
    }
  }
  return groups.filter(g => g.nodes.length > 0)
}

function groupContainsPath(group: Group, pathname: string): boolean {
  const check = (node: Node): boolean => {
    if (node.type === 'page' && node.url === pathname) return true
    if (node.type === 'folder' && node.children?.some(check)) return true
    return false
  }
  return group.nodes.some(check)
}

const STORAGE_PREFIX = 'solvela-docs-sidebar:'

function useSectionState(label: string, defaultOpen: boolean) {
  const [isOpen, setIsOpen] = useState(defaultOpen)
  const [mounted, setMounted] = useState(false)

  useEffect(() => {
    const stored = localStorage.getItem(`${STORAGE_PREFIX}${label}`)
    if (stored !== null) {
      setIsOpen(stored === 'true')
    }
    setMounted(true)
  }, [label])

  const toggle = () => {
    setIsOpen(prev => {
      const next = !prev
      localStorage.setItem(`${STORAGE_PREFIX}${label}`, String(next))
      return next
    })
  }

  return { isOpen, toggle, mounted }
}

interface DocsSidebarProps {
  tree: Root
}

export function DocsSidebar({ tree }: DocsSidebarProps) {
  const pathname = usePathname()
  const groups = groupNodes(tree.children)

  return (
    <aside className="hidden lg:block w-64 shrink-0">
      <nav className="sticky top-20 max-h-[calc(100vh-6rem)] overflow-y-auto pb-10 pr-2">
        {groups.map((group, idx) => (
          <SidebarGroup
            key={idx}
            group={group}
            pathname={pathname}
            isFirst={idx === 0}
          />
        ))}

        {/* External links */}
        <div className="mt-6 pt-4 border-t border-border space-y-0.5">
          {siteConfig.links.github && (
            <a
              href={siteConfig.links.github}
              target="_blank"
              rel="noopener noreferrer"
              className="flex items-center gap-2 py-1.5 px-2 text-[13px] text-muted-foreground hover:text-foreground transition-colors"
            >
              <Icon name="github" className="w-3.5 h-3.5" />
              GitHub
            </a>
          )}
        </div>
      </nav>
    </aside>
  )
}

interface SidebarGroupProps {
  group: Group
  pathname: string
  isFirst: boolean
}

function SidebarGroup({ group, pathname, isFirst }: SidebarGroupProps) {
  const hasActive = groupContainsPath(group, pathname)
  const { isOpen, toggle } = useSectionState(group.label ?? '__root__', true)

  // Ungrouped (no separator) — render flat
  if (group.label === null) {
    return (
      <div className={cn('space-y-0.5', !isFirst && 'pt-5')}>
        {group.nodes.map((node, i) => (
          <SidebarNode key={i} node={node} pathname={pathname} level={0} />
        ))}
      </div>
    )
  }

  return (
    <div className={cn(!isFirst && 'pt-5')}>
      <button
        type="button"
        onClick={toggle}
        aria-expanded={isOpen}
        className={cn(
          'w-full flex items-center justify-between mb-2 px-2 group',
          'text-[11px] font-medium uppercase tracking-widest font-mono',
          hasActive ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'
        )}
      >
        <span>{group.label}</span>
        <Icon
          name={isOpen ? 'chevron-down' : 'chevron-right'}
          className="w-3 h-3 opacity-60 group-hover:opacity-100 transition-opacity"
        />
      </button>
      {isOpen && (
        <div className="space-y-0.5">
          {group.nodes.map((node, i) => (
            <SidebarNode key={i} node={node} pathname={pathname} level={0} />
          ))}
        </div>
      )}
    </div>
  )
}

interface SidebarNodeProps {
  node: Node
  pathname: string
  level: number
}

function SidebarNode({ node, pathname, level }: SidebarNodeProps) {
  if (node.type === 'folder') {
    return (
      <div>
        <span className="block py-1.5 px-2 text-[13px] font-medium text-muted-foreground">
          {node.name}
        </span>
        {node.children && (
          <ul className="ml-2 mt-0.5 space-y-0.5 border-l border-border pl-3">
            {node.children.map((child, index) => (
              <SidebarNode key={index} node={child} pathname={pathname} level={level + 1} />
            ))}
          </ul>
        )}
      </div>
    )
  }

  if (node.type !== 'page') return null

  const isActive = pathname === node.url
  const isApiEndpoint = (node.url as string)?.includes('/api-reference/')
  const nodeName = typeof node.name === 'string' ? node.name : ''
  const httpMethod = isApiEndpoint ? getHttpMethod(nodeName) : null

  return (
    <Link
      href={node.url}
      className={cn(
        'flex items-center gap-2 py-1.5 px-2 text-[13px] transition-colors rounded',
        isActive
          ? 'text-foreground font-medium nav-item-active'
          : 'text-muted-foreground hover:text-foreground hover:bg-[var(--card)]/50'
      )}
    >
      {httpMethod && <HttpMethodBadge method={httpMethod} />}
      <span>{node.name}</span>
    </Link>
  )
}
