interface TierCard {
  name: string
  isDefault?: boolean
  description: string
  models: string[]
  useCase: string
}

const TIERS: TierCard[] = [
  {
    name: 'Eco',
    description: 'Cheapest capable model per tier.',
    models: ['DeepSeek Chat', 'Gemini Flash Lite', 'DeepSeek Reasoner'],
    useCase: 'High-volume, cost-sensitive workloads',
  },
  {
    name: 'Auto',
    isDefault: true,
    description: 'Balanced cost and quality.',
    models: ['Gemini Flash', 'Grok Code', 'Gemini Pro', 'Grok 4 Reasoning'],
    useCase: 'General-purpose, recommended for most agents',
  },
  {
    name: 'Premium',
    description: 'Best model regardless of cost.',
    models: ['GPT-4o', 'Claude Sonnet 4', 'Claude Opus 4', 'o3'],
    useCase: 'Complex reasoning, high-stakes decisions',
  },
]

export function TierCards() {
  return (
    <div className="not-prose my-12">
      <p className="eyebrow mb-3">Smart router</p>
      <h2 className="font-serif text-[2.25rem] font-medium text-[var(--heading-color)] mb-8 leading-tight tracking-tight">
        Routing profiles
      </h2>
      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {TIERS.map((tier) => (
          <div key={tier.name} className="terminal-card">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots">
                <span className={`terminal-card-dot ${tier.isDefault ? 'terminal-card-dot--accent' : ''}`} />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
              </span>
              <span className="ml-1">routing.{tier.name.toLowerCase()}</span>
              {tier.isDefault && (
                <span className="ml-auto text-[10px] tracking-wider text-[var(--accent-salmon)]">
                  default
                </span>
              )}
            </div>
            <div className="terminal-card-screen flex flex-col gap-5">
              <p className="font-serif text-[1.875rem] font-medium text-[var(--heading-color)] leading-tight">
                {tier.name}
              </p>

              <p className="font-serif text-[1.0625rem] text-[var(--foreground)]/65 leading-relaxed -mt-3">
                {tier.description}
              </p>

              <div>
                <p className="font-mono text-[11px] text-[var(--muted-foreground)] uppercase tracking-[0.12em] mb-2">
                  Models
                </p>
                <p className="font-sans text-[14px] text-[var(--foreground)] leading-relaxed">
                  {tier.models.join(' · ')}
                </p>
              </div>

              <div>
                <p className="font-mono text-[11px] text-[var(--muted-foreground)] uppercase tracking-[0.12em] mb-2">
                  Use case
                </p>
                <p className="font-serif text-[14px] text-[var(--foreground)]/70 leading-relaxed italic">
                  {tier.useCase}
                </p>
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
