import { EscrowSequence } from './escrow-sequence'

export function EscrowPanel() {
  return (
    <section
      aria-labelledby="escrow-heading"
      className="relative border-t border-border/60 bg-[var(--popover)]"
    >
      <EscrowSequence />
    </section>
  )
}
