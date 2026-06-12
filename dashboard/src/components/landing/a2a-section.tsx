import { Reveal } from '@/components/motion/primitives'
import { A2ADiagram } from './a2a-diagram'
import { DOCS_URL } from './config'

/**
 * Agent-to-agent section — the discovery story. An agent finds Solvela's
 * AgentCard, gets a payment-required task, signs, and receives artifacts,
 * with no human onboarding anywhere in the loop. Copy stays inside the
 * truth-verified claim set; the diagram labels come from the a2a module.
 */
export function A2ASection() {
  return (
    <section aria-labelledby="a2a-heading" className="border-t border-border/60">
      <div className="mx-auto max-w-[1320px] px-6 py-16 lg:py-24">
        <div className="grid items-center gap-12 lg:grid-cols-[minmax(0,0.46fr)_minmax(0,0.54fr)]">
          <div>
            <Reveal>
              <span className="eyebrow">agent-to-agent</span>
              <h2
                id="a2a-heading"
                className="display-wonk mt-3 max-w-[14ch] text-[var(--heading-color)]"
                style={{ fontSize: 'clamp(2rem, 4.6vw, 4rem)' }}
              >
                No signup form. An AgentCard.
              </h2>
            </Reveal>
            <Reveal index={1}>
              <p className="mt-6 max-w-[52ch] text-[15px] leading-[1.65] text-muted-foreground">
                Solvela speaks the A2A protocol. An agent fetches{' '}
                <code className="font-mono text-[13px] text-[var(--accent-gold)]">
                  /.well-known/agent.json
                </code>
                , sends a <code className="font-mono text-[13px]">message/send</code>,
                gets back a task that says exactly what it costs, signs a USDC
                payment, and collects its artifacts — discovery to settlement
                over one JSON-RPC surface, no human in the loop.
              </p>
              <a
                href={`${DOCS_URL}/docs/concepts/a2a`}
                className="mt-7 inline-flex items-center gap-2 font-mono text-[12px] uppercase tracking-[0.18em] text-[var(--accent-salmon)] hover:text-[var(--accent-salmon-hover)]"
              >
                a2a docs →
              </a>
            </Reveal>
          </div>
          <Reveal index={2}>
            <A2ADiagram />
          </Reveal>
        </div>
      </div>
    </section>
  )
}
