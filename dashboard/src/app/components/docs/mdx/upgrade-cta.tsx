import Link from 'next/link'

export function UpgradeCta() {
  return (
    <div className="not-prose my-10 terminal-card">
      <div className="terminal-card-titlebar">
        <span className="terminal-card-dots">
          <span className="terminal-card-dot terminal-card-dot--accent" />
          <span className="terminal-card-dot" />
          <span className="terminal-card-dot" />
        </span>
        <span>enterprise.upgrade</span>
      </div>
      <div className="terminal-card-screen">
        <h3 className="font-serif text-[1.5rem] font-medium text-[var(--heading-color)] mb-3 leading-tight tracking-tight">
          Ready for production?
        </h3>
        <p className="text-[var(--muted-foreground)] text-[15px] leading-relaxed mb-6">
          Solvela Enterprise is $49.99/mo + 5% per route. Includes everything on
          this page plus the dashboard.
        </p>
        <Link
          href="https://solvela.ai/pricing"
          target="_blank"
          rel="noopener noreferrer"
          className="inline-flex items-center gap-1.5 px-4 py-2 text-[14px] font-medium bg-[#1F1E1D] text-[#FAF9F5] border border-[#1F1E1D] rounded-md hover:opacity-90 transition-opacity"
        >
          View pricing →
        </Link>
      </div>
    </div>
  )
}
