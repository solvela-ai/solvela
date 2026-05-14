import Link from 'next/link'

interface LinkItem {
  label: string
  href: string
  description?: string
}

interface LinkMapProps {
  eyebrow?: string
  heading?: string
  links: LinkItem[]
}

export function LinkMap({
  eyebrow = 'Documentation',
  heading = 'Explore the docs',
  links,
}: LinkMapProps) {
  if (!links || links.length === 0) return null
  return (
    <div className="not-prose my-12">
      <p className="eyebrow mb-3">{eyebrow}</p>
      <h2 className="font-serif text-[2.25rem] font-medium text-[var(--heading-color)] mb-8 leading-tight tracking-tight">
        {heading}
      </h2>
      <div className="grid grid-cols-1 md:grid-cols-3 gap-x-12 gap-y-10">
        {links.map((link) => (
          <div key={link.href}>
            <Link
              href={link.href}
              className="group inline-flex items-center gap-1.5 text-[17px] text-[var(--foreground)] hover:text-[var(--accent-salmon)] transition-colors font-serif font-medium"
            >
              <span>{link.label}</span>
              <span
                aria-hidden="true"
                className="text-[var(--muted-foreground)] group-hover:text-[var(--accent-salmon)] transition-colors"
              >
                →
              </span>
            </Link>
            {link.description ? (
              <p className="mt-2 text-[15px] leading-relaxed text-[var(--muted-foreground)]">
                {link.description}
              </p>
            ) : null}
          </div>
        ))}
      </div>
    </div>
  )
}
