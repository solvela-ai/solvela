import { source } from '@/lib/source'
import { DocsSidebar } from '@/app/components/docs/docs-sidebar'
import { DocsHeader } from '@/app/components/docs/docs-header'
import { siteConfig } from '@/lib/theme-config'

export default function DocsLayout({
  children,
}: {
  children: React.ReactNode
}) {
  const tree = source.pageTree

  return (
    <div className="min-h-screen flex flex-col">
      {/* Header with inline nav tabs + mobile navigation */}
      <DocsHeader tree={tree} />

      {/* Main content — flush-left sidebar, full-width spread */}
      <div className="flex-1">
        <div className="flex gap-10 px-4 sm:px-6 lg:px-8 py-6 sm:py-8 lg:py-10">
          <DocsSidebar tree={tree} />
          <main className="flex-1 min-w-0">
            {children}
          </main>
        </div>
      </div>

      {/* Footer — full width */}
      <footer className="border-t border-border mt-16">
        <div className="px-4 sm:px-6 lg:px-8 py-6">
          <div className="flex flex-col sm:flex-row justify-between items-center gap-3">
            <p className="text-xs text-muted-foreground">
              {siteConfig.footer.companyName}
            </p>
            <div className="flex items-center gap-3">
              {siteConfig.footer.links.map((link) => (
                <a
                  key={link.href}
                  href={link.href}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-xs text-muted-foreground hover:text-foreground transition-colors"
                >
                  {link.label}
                </a>
              ))}
            </div>
          </div>
        </div>
      </footer>
    </div>
  )
}
