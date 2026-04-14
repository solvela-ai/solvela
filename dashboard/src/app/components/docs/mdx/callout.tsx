import { cn } from '@/lib/utils'
import { Icon } from '@/lib/icons'

interface CalloutProps {
  children: React.ReactNode
  title?: string
}

const calloutStyles = {
  info: {
    container: 'bg-[var(--card)] border-[var(--border)]',
    icon: 'text-[var(--heading-color)]',
    title: 'text-[var(--heading-color)]',
    content: 'text-[var(--foreground)]/80',
  },
  tip: {
    container: 'bg-[#65BB30]/5 border-[#65BB30]/20',
    icon: 'text-[#65BB30]',
    title: 'text-[#65BB30]',
    content: 'text-[var(--foreground)]/80',
  },
  warning: {
    container: 'bg-[#FE8181]/5 border-[#FE8181]/20',
    icon: 'text-[#FE8181]',
    title: 'text-[#FE8181]',
    content: 'text-[var(--foreground)]/80',
  },
  note: {
    container: 'bg-[var(--card)] border-[var(--border)]',
    icon: 'text-[var(--muted-foreground)]',
    title: 'text-[var(--heading-color)]',
    content: 'text-[var(--foreground)]/80',
  },
  check: {
    container: 'bg-[#65BB30]/5 border-[#65BB30]/20',
    icon: 'text-[#65BB30]',
    title: 'text-[#65BB30]',
    content: 'text-[var(--foreground)]/80',
  },
}

function createCallout(type: keyof typeof calloutStyles, iconName: string, defaultTitle: string) {
  return function Callout({ children, title }: CalloutProps) {
    const styles = calloutStyles[type]
    return (
      <div className={cn('my-8 rounded-lg border p-4', styles.container)}>
        <div className="flex gap-3">
          <div className={cn('mt-0.5 shrink-0', styles.icon)}>
            <Icon name={iconName} className="w-5 h-5" />
          </div>
          <div className="flex-1 min-w-0">
            {Boolean(title || defaultTitle) && (
              <p className={cn('font-semibold mb-1', styles.title)}>
                {title || defaultTitle}
              </p>
            )}
            <div className={cn('text-sm [&>p]:m-0', styles.content)}>
              {children}
            </div>
          </div>
        </div>
      </div>
    )
  }
}

export const Info = createCallout('info', 'info', 'Info')
export const Tip = createCallout('tip', 'tip', 'Tip')
export const Warning = createCallout('warning', 'warning', 'Warning')
export const Note = createCallout('note', 'note', 'Note')
export const Check = createCallout('check', 'check-circle', '')
