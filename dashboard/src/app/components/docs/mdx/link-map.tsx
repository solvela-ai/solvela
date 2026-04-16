import Link from 'next/link'

interface LinkItem {
  label: string
  href: string
}

interface LinkColumn {
  heading: string
  links: LinkItem[]
}

const COLUMNS: LinkColumn[] = [
  {
    heading: 'Get started',
    links: [
      { label: 'Quickstart', href: '/docs/quickstart' },
      { label: 'Architecture', href: '/docs/architecture' },
      { label: 'Request Flow', href: '/docs/request-flow' },
      { label: 'Payment Flow', href: '/docs/payment-flow' },
    ],
  },
  {
    heading: 'Protocol',
    links: [
      { label: 'x402 Overview', href: '/docs/x402-overview' },
      { label: 'Smart Router', href: '/docs/architecture#smart-router' },
    ],
  },
  {
    heading: 'Integrate',
    links: [
      { label: 'TypeScript SDK', href: '/docs/sdk-typescript' },
      { label: 'Python SDK', href: '/docs/sdk-python' },
      { label: 'Rust SDK', href: '/docs/sdk-rust' },
      { label: 'Go SDK', href: '/docs/sdk-go' },
    ],
  },
]

export function LinkMap() {
  return (
    <div className="not-prose my-12">
      <p className="eyebrow mb-3">Documentation</p>
      <h2 className="font-serif text-[2.25rem] font-medium text-[var(--heading-color)] mb-8 leading-tight tracking-tight">
        Explore the docs
      </h2>
      <div className="grid grid-cols-1 md:grid-cols-3 gap-x-12 gap-y-10">
        {COLUMNS.map((col) => (
          <div key={col.heading}>
            <p className="font-serif text-[17px] text-[var(--heading-color)] mb-4 font-medium">
              {col.heading}
            </p>
            <ul className="space-y-3">
              {col.links.map((link) => (
                <li key={link.href}>
                  <Link
                    href={link.href}
                    className="group inline-flex items-center gap-1.5 text-[17px] text-[var(--foreground)]/75 hover:text-[var(--heading-color)] transition-colors font-serif"
                  >
                    <span>{link.label}</span>
                    <span
                      aria-hidden="true"
                      className="text-[var(--muted-foreground)] group-hover:text-[var(--accent-salmon)] transition-colors"
                    >
                      →
                    </span>
                  </Link>
                </li>
              ))}
            </ul>
          </div>
        ))}
      </div>
    </div>
  )
}
