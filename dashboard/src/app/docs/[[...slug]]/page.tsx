import { source } from '@/lib/source'
import { notFound } from 'next/navigation'
import { DocsTOC } from '@/app/components/docs/docs-toc'
import { DocsPager } from '@/app/components/docs/docs-pager'
import { getMDXComponents } from '@/app/components/docs/mdx'
import { findNeighbour } from 'fumadocs-core/page-tree'
import type { Metadata } from 'next'
import type { Root, Node } from 'fumadocs-core/page-tree'
import { getSiteUrl } from '@/lib/theme-config'

interface PageProps {
  params: Promise<{ slug?: string[] }>
}

// Find the section separator that precedes this page in the tree
function findSectionName(tree: Root, pageUrl: string): string {
  let lastSeparator = 'Documentation'

  function traverse(nodes: Node[]): string | null {
    for (const node of nodes) {
      if (node.type === 'separator') {
        // node.name can be ReactNode, convert to string safely
        lastSeparator = typeof node.name === 'string' ? node.name : 'Documentation'
      } else if (node.type === 'page' && node.url === pageUrl) {
        return lastSeparator
      } else if (node.type === 'folder' && node.children) {
        const result = traverse(node.children)
        if (result) return result
      }
    }
    return null
  }

  return traverse(tree.children) || lastSeparator
}

export default async function DocsPage({ params }: PageProps) {
  const { slug } = await params
  const page = source.getPage(slug)

  if (!page) notFound()

  const MDXContent = page.data.body
  const toc = page.data.toc

  // Get prev/next navigation using fumadocs utility
  const tree = source.pageTree
  const neighbours = findNeighbour(tree, page.url)

  // Find section name for the header banner
  const sectionName = findSectionName(tree, page.url)

  return (
    <div className="flex gap-12 xl:gap-16 w-full">
      {/* Main content */}
      <article className="flex-1 min-w-0">
        {/* Header banner */}
        <header className="mb-10 pb-8 border-b border-border">
          <p className="text-[11px] text-muted-foreground font-medium mb-3 uppercase tracking-[0.15em] font-mono">
            {sectionName}
          </p>
          <h1 className="inline-title font-bold">
            {page.data.title}
          </h1>
          {page.data.description && (
            <p className="mt-4 text-base text-muted-foreground leading-relaxed">
              {page.data.description}
            </p>
          )}
        </header>

        {/* MDX content */}
        <div className="prose prose-slate dark:prose-invert max-w-none">
          <MDXContent components={getMDXComponents()} />
        </div>

        <DocsPager
          previous={neighbours.previous ? {
            name: typeof neighbours.previous.name === 'string' ? neighbours.previous.name : 'Previous',
            url: neighbours.previous.url
          } : undefined}
          next={neighbours.next ? {
            name: typeof neighbours.next.name === 'string' ? neighbours.next.name : 'Next',
            url: neighbours.next.url
          } : undefined}
        />
      </article>

      {/* Table of contents */}
      <DocsTOC toc={toc} />
    </div>
  )
}

export async function generateStaticParams() {
  return source.generateParams()
}

export async function generateMetadata({ params }: PageProps): Promise<Metadata> {
  const { slug } = await params
  const page = source.getPage(slug)

  if (!page) return {}

  const tree = source.pageTree
  const section = findSectionName(tree, page.url)
  const title = page.data.title
  const description = page.data.description

  return {
    title,
    description,
  }
}
