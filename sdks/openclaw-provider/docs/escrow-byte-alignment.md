# Escrow byte-alignment limitation

## TL;DR

This plugin's `wrapStreamFn` body sends a **stub probe body** to the Solvela
gateway to obtain the 402 `PaymentRequired` envelope, then signs payment and
forwards the real request through pi-ai's transport. The probe body matches
the real request on `model.id` and cost-sensitive `options` fields
(`max_tokens`, `temperature`, `top_p`) but **does not byte-match the
serialized request body**.

For **direct mode** (the plugin's default and Solvela's strategic mode), this
is fine — the gateway prices upfront from `model` + `max_tokens` ceiling, and
the on-chain settlement is a simple USDC-SPL `TransferChecked` that does not
depend on the request body.

For **escrow mode**, the gateway derives the escrow PDA from
`[b"escrow", agent.key().as_ref(), &service_id]`, where `service_id` may be
hashed from request shape. A stub probe body could compute a different
`service_id` than the real call, breaking PDA derivation and causing the
gateway to look up the deposit at the wrong PDA.

## Why we ship the stub-probe approach anyway

1. **Direct mode is the documented default** (`SOLVELA_SIGNING_MODE=direct`,
   see `src/index.ts`). Most users will never hit the escrow byte-alignment
   path.
2. **Reconstructing the real serialized body is brittle.** pi-ai's transport
   serializes `context.messages` and `options` internally, and the
   serialization may differ across provider apis (`openai-completions`,
   `anthropic-messages`, etc.) and pi-ai versions. Replicating that
   serialization in the plugin would couple Solvela to pi-ai internals
   tightly enough that any pi-ai update could silently drift the probe body
   from the real body (a class of regression we already saw in
   `pi-coding-agent@0.63.0` — see openclaw#55760, #55816, #57860).
3. **F4 settle (planned, separate PR) reduces the urgency.** Escrow
   accounting can be done after the stream completes by claiming the actual
   used amount from the deposit, rather than requiring byte-perfect
   probe alignment at request time.

## When this becomes a problem

Escrow mode + a gateway that hashes the request body into `service_id`. The
plugin will deposit at one PDA and the gateway will check a different PDA;
the call fails with "deposit not found" or similar.

## Mitigations / future work

If escrow mode becomes a first-class requirement (i.e. direct mode is no
longer sufficient), the canonical path is to move payment interception from
`wrapStreamFn` to an undici `Dispatcher` registered via
`setGlobalDispatcher` (or threaded via `init.dispatcher` through OpenClaw's
`fetchWithRuntimeDispatcher`). The dispatcher sees the actual outbound
request bytes after pi-ai serialization, so the probe and real call are
guaranteed to share the same `service_id`.

That refactor is ~8–16h of work and was originally planned as PR 3 ("Path
B") but was deferred in favor of Path A' (this implementation) because:

- Direct mode is the strategic default for v1.
- F4 settle (separate PR) is the correct architectural answer for escrow
  accounting, not request-time byte alignment.
- Dispatcher implementation has its own slim-down risks against OpenClaw's
  evolving plugin SDK surface — we don't gain insurance for free.

If escrow byte-alignment becomes load-bearing, file a new issue referencing
this doc; the migration is contained to `src/index.ts` (drop `wrapStreamFn`
body; add `src/dispatcher.ts`; wire `setGlobalDispatcher` in `register`).

## References

- `src/index.ts` — `wrapStreamFn` body that does stub-probe signing
- `src/index.ts` — `buildProbeBody` helper
- Solana escrow PDA: `programs/escrow/` in the main solvela-ai/solvela repo
- 2026-05-13 OpenClaw due-diligence audit (project memory)
