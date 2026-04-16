interface FlowStep {
  number: string
  title: string
  description: string
}

const STEPS: FlowStep[] = [
  {
    number: '01',
    title: 'Request',
    description: 'Agent sends a chat request, gets back a 402 with cost breakdown.',
  },
  {
    number: '02',
    title: 'Pay',
    description: 'Agent signs a USDC-SPL transaction on Solana.',
  },
  {
    number: '03',
    title: 'Route',
    description: 'Gateway verifies payment, routes to optimal LLM, returns response.',
  },
]

export function FlowSteps() {
  return (
    <div className="not-prose my-12">
      <p className="eyebrow mb-3">How it works</p>
      <h2 className="font-serif text-[2.25rem] font-medium text-[var(--heading-color)] mb-8 leading-tight tracking-tight">
        Three steps, settled on-chain
      </h2>

      <div className="grid grid-cols-1 md:grid-cols-3 gap-4">
        {STEPS.map((step) => (
          <div key={step.number} className="terminal-card">
            <div className="terminal-card-titlebar">
              <span className="terminal-card-dots">
                <span className="terminal-card-dot terminal-card-dot--accent" />
                <span className="terminal-card-dot" />
                <span className="terminal-card-dot" />
              </span>
              <span className="ml-1">step.{step.number}</span>
            </div>
            <div className="terminal-card-screen">
              <p className="font-serif text-[1.625rem] font-medium text-[var(--heading-color)] leading-tight mb-3">
                {step.title}
              </p>
              <p className="font-serif text-[1.0625rem] text-[var(--foreground)]/65 leading-relaxed">
                {step.description}
              </p>
            </div>
          </div>
        ))}
      </div>
    </div>
  )
}
