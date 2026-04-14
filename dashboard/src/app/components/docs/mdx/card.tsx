import Link from 'next/link'
import Image from 'next/image'
import { cn } from '@/lib/utils'
import { Icon, hasIcon } from '@/lib/icons'

interface CardProps {
  title: string
  icon?: string
  image?: string
  href?: string
  featured?: boolean
  children?: React.ReactNode
}

export function Card({ title, icon, image, href, featured, children }: CardProps) {
  const showIcon = icon && hasIcon(icon)

  // Featured variant: app-window header with large icon
  if (featured && showIcon) {
    const content = (
      <div
        className={cn(
          'group block h-full rounded-lg border border-border bg-[#1F1E1D] overflow-hidden',
          'hover:bg-transparent transition-colors duration-150',
          href && 'cursor-pointer'
        )}
      >
        {/* Titlebar — card color, contrasts with the #1F1E1D card background */}
        <div className="flex items-center gap-2 px-4 h-9 bg-[var(--card)] border-b border-[rgba(153,153,153,0.15)]">
          <span className="w-[9px] h-[9px] rounded-full border border-[#FE8181]/40 bg-[#FE8181]/20" />
          <span className="w-[9px] h-[9px] rounded-full border border-[var(--foreground)]/15 bg-[var(--foreground)]/8" />
          <span className="w-[9px] h-[9px] rounded-full border border-[var(--foreground)]/15 bg-[var(--foreground)]/8" />
        </div>
        {/* Window view framed inside the card */}
        <div className="p-4">
          <div className="rounded-md overflow-hidden border border-[rgba(153,153,153,0.15)] bg-[var(--card)]">
            <div className="flex items-center justify-center py-12 text-[var(--foreground)] group-hover:text-[var(--heading-color)] transition-colors">
              <Icon name={icon!} className="w-12 h-12" />
            </div>
          </div>
        </div>
        {/* Card body */}
        <div className="px-6 py-5">
          <h3 className="text-lg font-semibold text-foreground mb-2">
            {title}
            {href && (
              <span className="inline-block ml-1.5 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground">&rarr;</span>
            )}
          </h3>
          {children && (
            <div className="text-[15px] text-muted-foreground leading-relaxed [&>p]:m-0">
              {children}
            </div>
          )}
        </div>
      </div>
    )

    if (href) {
      return <Link href={href} className="block h-full">{content}</Link>
    }
    return content
  }

  // Standard variant
  const content = (
    <div
      className={cn(
        'group block h-full px-5 py-4 rounded-lg border border-border bg-transparent',
        'hover:bg-[var(--card)] transition-colors duration-150',
        href && 'cursor-pointer'
      )}
    >
      {image ? (
        <div className="mb-3 flex items-center justify-center w-9 h-9 rounded-full bg-[#141413]">
          <div className="w-5 h-5 relative">
            <Image
              src={image}
              alt=""
              width={20}
              height={20}
              className="object-contain"
            />
          </div>
        </div>
      ) : showIcon && (
        <div className="mb-3 flex items-center justify-center w-9 h-9 rounded-full bg-[#141413] text-[var(--foreground)] group-hover:text-[var(--heading-color)] transition-colors">
          <Icon name={icon!} className="w-5 h-5" />
        </div>
      )}
      <h3 className="text-[15px] font-medium text-foreground mb-1.5">
        {title}
        {href && (
          <span className="inline-block ml-1.5 opacity-0 group-hover:opacity-100 transition-opacity text-muted-foreground">&rarr;</span>
        )}
      </h3>
      {children && (
        <div className="text-sm text-muted-foreground leading-relaxed [&>p]:m-0">
          {children}
        </div>
      )}
    </div>
  )

  if (href) {
    return <Link href={href} className="block h-full">{content}</Link>
  }

  return content
}

interface CardGroupProps {
  cols?: number
  children: React.ReactNode
}

export function CardGroup({ cols = 2, children }: CardGroupProps) {
  return (
    <div
      className={cn(
        'grid gap-4 my-8 auto-rows-fr',
        cols === 1 && 'grid-cols-1',
        cols === 2 && 'grid-cols-1 sm:grid-cols-2',
        cols === 3 && 'grid-cols-1 sm:grid-cols-2 lg:grid-cols-3',
        cols === 4 && 'grid-cols-1 sm:grid-cols-2 lg:grid-cols-4'
      )}
    >
      {children}
    </div>
  )
}
